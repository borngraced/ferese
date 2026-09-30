use std::io;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use ferese_config::theme::Snapshot;
use serde_json::{Value, json};

use crate::{Request, Response, VERSION, read_frame, write_frame};

pub fn socket_path() -> io::Result<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(|directory| PathBuf::from(directory).join("ferese/control.sock"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))
}

pub struct Connection {
    stream: UnixStream,
    id: u64,
}

impl Connection {
    pub fn connect() -> io::Result<Self> {
        let stream = UnixStream::connect(socket_path()?)?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        Ok(Self { stream, id: 0 })
    }

    pub fn cancellation(&self) -> io::Result<Cancellation> {
        self.stream.try_clone().map(Cancellation)
    }

    pub fn call(&mut self, command: &str, args: Value) -> Result<Value, String> {
        self.id = self.id.wrapping_add(1);
        let request = Request {
            version: VERSION,
            id: self.id,
            kind: "command".into(),
            command: command.into(),
            args,
        };
        write_frame(&mut self.stream, &request).map_err(|e| e.to_string())?;
        let response: Response = read_frame(&mut self.stream).map_err(|e| e.to_string())?;
        if response.id != self.id || response.version != VERSION {
            return Err("Unexpected theme IPC response".into());
        }
        if let Some(error) = response.error {
            return Err(error.message);
        }
        response.result.ok_or_else(|| "Missing theme snapshot".into())
    }

    pub fn get(&mut self) -> Result<Snapshot, String> {
        let snapshot: Snapshot =
            serde_json::from_value(self.call("theme-get", json!({}))?).map_err(|e| e.to_string())?;
        if snapshot.version != ferese_config::theme::SCHEMA_VERSION {
            return Err("Unsupported theme snapshot version".into());
        }
        Ok(snapshot)
    }

    pub fn watch(&mut self, revision: u64) -> Result<Snapshot, String> {
        self.stream.set_read_timeout(None).map_err(|e| e.to_string())?;
        let snapshot: Snapshot =
            serde_json::from_value(self.call("theme-watch", json!({"since": revision}))?).map_err(|e| e.to_string())?;
        if snapshot.version != ferese_config::theme::SCHEMA_VERSION {
            return Err("Unsupported theme snapshot version".into());
        }
        Ok(snapshot)
    }
}

pub struct Cancellation(UnixStream);

impl Drop for Cancellation {
    fn drop(&mut self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

pub fn current() -> Snapshot {
    Connection::connect()
        .ok()
        .and_then(|mut connection| connection.get().ok())
        .unwrap_or_default()
}
