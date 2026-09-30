#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct Entry {
    pub keys: String,
    pub description: String,
}

pub(super) fn load() -> Result<Vec<Entry>, String> {
    let entries: Vec<Entry> =
        serde_json::from_value(crate::compositor_ipc::call("get-keybindings")?)
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
