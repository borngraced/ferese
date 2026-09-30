//! Watch the parent directory so atomic editor saves keep working.
use std::path::PathBuf;
use std::time::Duration;

use cosmic::iced::futures::{SinkExt, Stream};
use notify::{RecursiveMode, Watcher};

use crate::Message;
use crate::store::Snapshot;

pub fn changes(path: &PathBuf) -> impl Stream<Item = Message> + use<> {
    let path = path.clone();

    cosmic::iced::stream::channel(1, async move |mut output| {
        let (send, mut events) = tokio::sync::mpsc::channel(1);
        let target = path.clone();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event
                && !event.kind.is_access()
                && event.paths.iter().any(|p| p == &target)
            {
                let _ = send.try_send(());
            }
        });
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                let _ = output.send(Message::ExternalConfig(Err(error.to_string()))).await;
                return;
            }
        };

        if let Err(error) = watcher.watch(
            path.parent().unwrap_or(std::path::Path::new(".")),
            RecursiveMode::NonRecursive,
        ) {
            let _ = output.send(Message::ExternalConfig(Err(error.to_string()))).await;
            return;
        }

        // Read once after installing the watch to cover the startup race.
        loop {
            let file = path.clone();
            let snapshot = tokio::task::spawn_blocking(move || Snapshot::read(&file))
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            if output.send(Message::ExternalConfig(snapshot)).await.is_err() {
                return;
            }
            if events.recv().await.is_none() {
                return;
            }
            while tokio::time::timeout(Duration::from_millis(150), events.recv())
                .await
                .is_ok()
            {}
        }
    })
}

#[cfg(test)]
mod tests {
    use cosmic::iced::futures::{StreamExt, pin_mut};

    use super::*;

    #[test]
    fn watches_atomic_replacement_and_retains_subscription_after_bad_kdl() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("config.kdl");
                std::fs::write(&path, "animations {\n    speed 1\n}\n").unwrap();
                let stream = changes(&path);
                pin_mut!(stream);
                assert!(matches!(stream.next().await, Some(Message::ExternalConfig(Ok(_)))));
                for source in ["bad [", "animations {\n    speed 0.75\n}\n"] {
                    let temporary = directory.path().join("replacement");
                    std::fs::write(&temporary, source).unwrap();
                    std::fs::rename(temporary, &path).unwrap();
                    let message = tokio::time::timeout(Duration::from_secs(3), stream.next())
                        .await
                        .unwrap()
                        .unwrap();
                    match message {
                        Message::ExternalConfig(Ok(snapshot)) => {
                            assert_eq!(snapshot.source, source)
                        }
                        Message::ExternalConfig(Err(_)) => assert_eq!(source, "bad ["),
                        _ => panic!("unexpected watcher message"),
                    }
                }
            });
    }
}
