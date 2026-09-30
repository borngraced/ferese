use std::{os::unix::net::UnixStream, path::PathBuf, time::Duration};

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct Entry {
    pub keys: String,
    pub description: String,
}

pub(super) fn load() -> Result<Vec<Entry>, String> {
    let path = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or("Missing runtime directory")?
        .join("ferese/control.sock");
    let mut stream = UnixStream::connect(path).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    let request = ferese_ipc::Request {
        version: ferese_ipc::VERSION,
        id: 1,
        kind: "command".into(),
        command: "get-keybindings".into(),
        args: serde_json::json!({}),
    };
    ferese_ipc::write_frame(&mut stream, &request).map_err(|error| error.to_string())?;
    let response: ferese_ipc::Response =
        ferese_ipc::read_frame(&mut stream).map_err(|error| error.to_string())?;
    if let Some(error) = response.error {
        return Err(error.message);
    }
    let entries: Vec<Entry> = serde_json::from_value(response.result.ok_or("Missing keybindings")?)
        .map_err(|error| error.to_string())?;
    if entries.len() > 256
        || entries
            .iter()
            .any(|entry| entry.keys.len() > 256 || entry.description.len() > 512)
    {
        return Err("Invalid keybinding guide".into());
    }
    Ok(entries)
}
