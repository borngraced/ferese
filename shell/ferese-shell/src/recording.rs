//! The bar only controls the recorder and consumes bounded status messages.
//! Encoding and portal negotiation run in the separate ferese-record process.
use serde::Deserialize;
use std::{
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    process::{ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Instant,
};

#[derive(Debug, Default)]
pub enum State {
    #[default]
    Idle,
    Selecting,
    Recording(Instant),
    Saving,
    Saved(PathBuf),
    Error(String),
}
#[derive(Deserialize)]
struct Update {
    state: String,
    message: Option<String>,
    path: Option<PathBuf>,
}
enum Event {
    Update(Update),
    Exited,
}
#[derive(Default)]
pub struct Recorder {
    pub state: State,
    control: Option<ChildStdin>,
    updates: Option<Receiver<Event>>,
}
impl Recorder {
    pub fn label(&self) -> &'static str {
        match self.state {
            State::Selecting => "Cancel display selection",
            State::Recording(_) => "Stop recording",
            State::Saving => "Saving recording",
            _ => "Record a display",
        }
    }

    pub fn busy(&self) -> bool {
        self.updates.is_some()
    }

    pub fn elapsed(&self) -> Option<String> {
        let State::Recording(start) = self.state else {
            return None;
        };
        let seconds = start.elapsed().as_secs();
        Some(format!("{:02}:{:02}", seconds / 60, seconds % 60))
    }

    pub fn start(&mut self) {
        if self.busy() {
            return;
        }

        let program = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("ferese-record")))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("ferese-record"));

        match Command::new(program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(mut child) => {
                self.control = child.stdin.take();
                let output = child.stdout.take().unwrap();
                let (send, receive) = mpsc::sync_channel(8);
                self.updates = Some(receive);
                self.state = State::Selecting;

                std::thread::spawn(move || {
                    let mut output = BufReader::new(output);

                    loop {
                        let mut line = String::new();
                        match output.by_ref().take(65537).read_line(&mut line) {
                            Ok(0) | Err(_) => break,
                            Ok(_) if line.len() > 65536 => break,
                            Ok(_) => {
                                if let Ok(update) = serde_json::from_str::<Update>(&line)
                                    && send.send(Event::Update(update)).is_err()
                                {
                                    break;
                                }
                            }
                        }
                    }

                    let _ = child.wait();
                    let _ = send.send(Event::Exited);
                });
            }
            Err(error) => {
                let message = format!("Cannot start recorder: {error}");
                notify("Recording unavailable", message.clone());
                self.state = State::Error(message);
            }
        }
    }

    pub fn stop(&mut self) {
        if self.busy() && !matches!(self.state, State::Saving) {
            // EOF requests EOS/finalization. Never terminate the encoder abruptly.
            self.control.take();
            self.state = State::Saving;
        }
    }

    pub fn poll(&mut self) {
        let mut done = false;
        if let Some(updates) = &self.updates {
            while let Ok(event) = updates.try_recv() {
                match event {
                    Event::Update(update) => match update.state.as_str() {
                        "selecting" if self.control.is_some() => self.state = State::Selecting,
                        "recording" if self.control.is_some() => {
                            self.state = State::Recording(Instant::now())
                        }
                        "saving" => self.state = State::Saving,
                        "saved" => {
                            self.state = update.path.map(State::Saved).unwrap_or_else(|| {
                                State::Error("Recorder returned no saved file".into())
                            })
                        }
                        "cancelled" => self.state = State::Idle,
                        "error" => {
                            self.state = State::Error(
                                update.message.unwrap_or_else(|| "Recording failed".into()),
                            )
                        }
                        _ => (),
                    },
                    Event::Exited => {
                        if matches!(
                            self.state,
                            State::Selecting | State::Recording(_) | State::Saving
                        ) {
                            self.state = State::Error("The recorder stopped unexpectedly".into());
                        }
                        done = true;
                    }
                }
            }
        }

        if done {
            match &self.state {
                State::Saved(path) => notify("Recording saved", path.display().to_string()),
                State::Error(message) => notify("Recording failed", message.clone()),
                _ => (),
            }
            self.control.take();
            self.updates.take();
        }
    }
}

// Use the normal notification center for results, without an extra recording menu.
fn notify(title: &'static str, body: String) {
    std::thread::spawn(move || {
        let result = (|| -> zbus::Result<()> {
            let connection = zbus::blocking::connection::Builder::session()?
                .method_timeout(std::time::Duration::from_secs(3))
                .build()?;
            let proxy = zbus::blocking::Proxy::new(
                &connection,
                "org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
            )?;
            let _: u32 = proxy.call(
                "Notify",
                &(
                    "Ferese",
                    0u32,
                    "ferese",
                    title,
                    body,
                    Vec::<String>::new(),
                    std::collections::HashMap::<String, zbus::zvariant::OwnedValue>::new(),
                    -1i32,
                ),
            )?;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("ferese-shell: recording notification: {error}");
        }
    });
}
