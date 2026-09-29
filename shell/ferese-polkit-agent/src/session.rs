use crate::PromptEvent;
use glib::gobject_ffi;
use libloading::Library;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    io::{BufRead, BufReader, BufWriter, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
    },
    time::Duration,
};
use zeroize::Zeroize;

enum SessionEvent {
    Request(String, bool),
    Info(String),
    Error(String),
    Completed(bool),
}

type Object = *mut c_void;

struct Api {
    _agent: Library,
    _polkit: Library,
    user_new: unsafe extern "C" fn(c_int) -> Object,
    session_new: unsafe extern "C" fn(Object, *const c_char) -> Object,
    initiate: unsafe extern "C" fn(Object),
    response: unsafe extern "C" fn(Object, *const c_char),
    cancel: unsafe extern "C" fn(Object),
}

impl Api {
    fn load() -> Result<Self, String> {
        unsafe {
            let agent = Library::new("libpolkit-agent-1.so.0").map_err(|e| e.to_string())?;
            let polkit = Library::new("libpolkit-gobject-1.so.0").map_err(|e| e.to_string())?;
            let user_new = *polkit
                .get(b"polkit_unix_user_new\0")
                .map_err(|e| e.to_string())?;
            let session_new = *agent
                .get(b"polkit_agent_session_new\0")
                .map_err(|e| e.to_string())?;
            let initiate = *agent
                .get(b"polkit_agent_session_initiate\0")
                .map_err(|e| e.to_string())?;
            let response = *agent
                .get(b"polkit_agent_session_response\0")
                .map_err(|e| e.to_string())?;
            let cancel = *agent
                .get(b"polkit_agent_session_cancel\0")
                .map_err(|e| e.to_string())?;

            Ok(Self {
                _agent: agent,
                _polkit: polkit,
                user_new,
                session_new,
                initiate,
                response,
                cancel,
            })
        }
    }
}

pub fn check() -> Result<(), String> {
    Api::load().map(|_| ())
}

fn username(uid: u32) -> String {
    let output = Command::new("getent")
        .args(["passwd", &uid.to_string()])
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|entry| entry.split(':').next().map(str::to_owned))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| uid.to_string())
}

fn send(writer: &mut BufWriter<ChildStdin>, event: &PromptEvent) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, event).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())
}

fn prompt_process(
    message: String,
    user: String,
) -> Result<(Child, BufWriter<ChildStdin>, mpsc::Receiver<PromptEvent>), String> {
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--prompt")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdin = child.stdin.take().ok_or("Prompt has no input")?;
    let stdout = child.stdout.take().ok_or("Prompt has no output")?;
    let mut writer = BufWriter::new(stdin);
    send(&mut writer, &PromptEvent::Start { message, user })?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(mut line) = line else { break };
            if let Ok(event) = serde_json::from_str(&line) {
                line.zeroize();
                if tx.send(event).is_err() {
                    break;
                }
            } else {
                line.zeroize();
            }
        }
    });
    Ok((child, writer, rx))
}

unsafe extern "C" fn request(
    _session: Object,
    prompt: *const c_char,
    echo: c_int,
    data: *mut c_void,
) {
    let tx = unsafe { &*(data as *const Sender<SessionEvent>) };
    let prompt = unsafe { CStr::from_ptr(prompt) }
        .to_string_lossy()
        .into_owned();
    let _ = tx.send(SessionEvent::Request(prompt, echo != 0));
}

unsafe extern "C" fn info(_session: Object, text: *const c_char, data: *mut c_void) {
    let tx = unsafe { &*(data as *const Sender<SessionEvent>) };
    let text = unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned();
    let _ = tx.send(SessionEvent::Info(text));
}

unsafe extern "C" fn error(_session: Object, text: *const c_char, data: *mut c_void) {
    let tx = unsafe { &*(data as *const Sender<SessionEvent>) };
    let text = unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned();
    let _ = tx.send(SessionEvent::Error(text));
}

unsafe extern "C" fn completed(_session: Object, success: c_int, data: *mut c_void) {
    let tx = unsafe { &*(data as *const Sender<SessionEvent>) };
    let _ = tx.send(SessionEvent::Completed(success != 0));
}

unsafe fn connect(
    session: Object,
    name: &'static CStr,
    callback: gobject_ffi::GCallback,
    data: *mut c_void,
) {
    unsafe {
        gobject_ffi::g_signal_connect_data(
            session.cast(),
            name.as_ptr(),
            callback,
            data,
            None,
            gobject_ffi::G_CONNECT_DEFAULT,
        );
    }
}

fn run_session(
    api: &Api,
    uid: u32,
    cookie: &CString,
    context: &glib::MainContext,
    writer: &mut BufWriter<ChildStdin>,
    input: &mpsc::Receiver<PromptEvent>,
    cancelled: &AtomicBool,
    child: &mut Child,
) -> Result<Option<bool>, String> {
    let identity = unsafe { (api.user_new)(uid as c_int) };
    if identity.is_null() {
        return Err("Could not create polkit identity".into());
    }
    let session = unsafe { (api.session_new)(identity, cookie.as_ptr()) };
    unsafe { gobject_ffi::g_object_unref(identity.cast()) };
    if session.is_null() {
        return Err("Could not create polkit session".into());
    }

    let (tx, rx) = mpsc::channel();
    let signal_data = Box::into_raw(Box::new(tx)).cast();
    unsafe {
        connect(
            session,
            c"request",
            Some(std::mem::transmute::<
                unsafe extern "C" fn(Object, *const c_char, c_int, *mut c_void),
                unsafe extern "C" fn(),
            >(
                request as unsafe extern "C" fn(Object, *const c_char, c_int, *mut c_void),
            )),
            signal_data,
        );
        connect(
            session,
            c"show-info",
            Some(std::mem::transmute::<
                unsafe extern "C" fn(Object, *const c_char, *mut c_void),
                unsafe extern "C" fn(),
            >(
                info as unsafe extern "C" fn(Object, *const c_char, *mut c_void),
            )),
            signal_data,
        );
        connect(
            session,
            c"show-error",
            Some(std::mem::transmute::<
                unsafe extern "C" fn(Object, *const c_char, *mut c_void),
                unsafe extern "C" fn(),
            >(
                error as unsafe extern "C" fn(Object, *const c_char, *mut c_void),
            )),
            signal_data,
        );
        connect(
            session,
            c"completed",
            Some(std::mem::transmute::<
                unsafe extern "C" fn(Object, c_int, *mut c_void),
                unsafe extern "C" fn(),
            >(
                completed as unsafe extern "C" fn(Object, c_int, *mut c_void),
            )),
            signal_data,
        );

        (api.initiate)(session);
    }

    let mut outcome = None;
    let mut stopped = false;
    while outcome.is_none() {
        while context.pending() {
            context.iteration(false);
        }
        while let Ok(event) = rx.try_recv() {
            match event {
                SessionEvent::Request(prompt, echo) => {
                    send(writer, &PromptEvent::Request { prompt, echo })?;
                }
                SessionEvent::Info(text) => send(writer, &PromptEvent::Info { text })?,
                SessionEvent::Error(text) => send(writer, &PromptEvent::Error { text })?,
                SessionEvent::Completed(success) => outcome = Some(success),
            }
        }
        if outcome.is_some() {
            break;
        }
        while let Ok(event) = input.try_recv() {
            match event {
                PromptEvent::Response { mut value } => {
                    if let Ok(response) = CString::new(value.as_str()) {
                        unsafe { (api.response)(session, response.as_ptr()) };
                    }
                    value.zeroize();
                }
                PromptEvent::Cancel => cancelled.store(true, Ordering::Release),
                _ => {}
            }
        }
        if !stopped
            && (cancelled.load(Ordering::Acquire)
                || child.try_wait().map_err(|e| e.to_string())?.is_some())
        {
            unsafe { (api.cancel)(session) };
            stopped = true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    unsafe {
        gobject_ffi::g_object_unref(session.cast());
        drop(Box::from_raw(signal_data as *mut Sender<SessionEvent>));
    }
    Ok(outcome.filter(|_| !stopped))
}

pub fn authenticate(
    uid: u32,
    message: String,
    cookie: String,
    cancelled: Arc<AtomicBool>,
) -> Result<bool, String> {
    let api = Api::load()?;
    let cookie = CString::new(cookie).map_err(|_| "Invalid authentication cookie")?;
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| {
            let (mut child, mut writer, input) = prompt_process(message, username(uid))?;
            let result = loop {
                match run_session(
                    &api,
                    uid,
                    &cookie,
                    &context,
                    &mut writer,
                    &input,
                    &cancelled,
                    &mut child,
                ) {
                    Ok(Some(true)) => break Ok(true),
                    Ok(Some(false)) if !cancelled.load(Ordering::Acquire) => {
                        if let Err(error) = send(
                            &mut writer,
                            &PromptEvent::Error {
                                text: "Password not accepted. Try again.".into(),
                            },
                        ) {
                            break Err(error);
                        }
                    }
                    Ok(_) => break Ok(false),
                    Err(error) => break Err(error),
                }
            };
            let _ = child.kill();
            let _ = child.wait();
            result
        })
        .map_err(|error| error.to_string())?
}
