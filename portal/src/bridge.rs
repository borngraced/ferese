use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value as Json;

#[derive(Clone)]
pub(crate) struct Bridge(Arc<BridgeInner>);

struct BridgeInner {
    stream: std::sync::Mutex<UnixStream>,
    shutdown: UnixStream,
}

impl Bridge {
    pub(crate) fn connect() -> Result<Self, String> {
        let path = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .ok_or("Missing runtime directory")?
            .join("ferese/control.sock");
        let stream = UnixStream::connect(path).map_err(|error| error.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|error| error.to_string())?;
        let shutdown = stream.try_clone().map_err(|error| error.to_string())?;
        Ok(Self(Arc::new(BridgeInner {
            stream: std::sync::Mutex::new(stream),
            shutdown,
        })))
    }

    pub(crate) async fn call(&self, command: &'static str, args: Json) -> Result<Json, String> {
        self.request(command, args, false).await
    }

    pub(crate) async fn wait(&self, since: u64) -> Result<Json, String> {
        self.request("session-watch", serde_json::json!({"since": since}), true)
            .await
    }

    async fn request(&self, command: &'static str, args: Json, wait: bool) -> Result<Json, String> {
        let bridge = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut stream = bridge.0.stream.lock().map_err(|_| "Compositor connection failed")?;
            stream
                .set_read_timeout(if wait { None } else { Some(Duration::from_secs(2)) })
                .map_err(|error| error.to_string())?;
            let request = ferese_ipc::Request {
                version: ferese_ipc::VERSION,
                id: 1,
                kind: "command".into(),
                command: command.into(),
                args,
            };
            ferese_ipc::write_frame(&mut *stream, &request).map_err(|error| error.to_string())?;
            let response: ferese_ipc::Response =
                ferese_ipc::read_frame(&mut *stream).map_err(|error| error.to_string())?;
            if let Some(error) = response.error {
                return Err(error.message);
            }
            response.result.ok_or("Missing compositor response".into())
        })
        .await
        .map_err(|error| error.to_string())?
    }

    pub(crate) async fn capture_watch(&self, session: u64) -> Result<Json, String> {
        self.request("input-capture-watch", serde_json::json!({"session":session}), true)
            .await
    }

    pub(crate) async fn closed(&self) {
        use std::io::Read;
        let Ok(reader) = self.0.shutdown.try_clone() else {
            return;
        };
        if reader.set_nonblocking(true).is_err() {
            return;
        }
        let Ok(reader) = tokio::io::unix::AsyncFd::new(reader) else {
            return;
        };
        loop {
            let Ok(mut ready) = reader.readable().await else {
                return;
            };
            match ready.try_io(|reader| {
                let mut socket = reader.get_ref();
                socket.read(&mut [0_u8; 1])
            }) {
                Ok(_) => return,
                Err(_) => continue,
            }
        }
    }

    pub(crate) fn close(&self) {
        let _ = self.0.shutdown.shutdown(std::net::Shutdown::Both);
    }
}
