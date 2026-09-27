use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationConfig {
    pub show_popups: bool,
    pub do_not_disturb: bool,
    pub timeout_ms: u32,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            show_popups: true,
            do_not_disturb: false,
            timeout_ms: 6000,
        }
    }
}

impl NotificationConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(1000..=30000).contains(&self.timeout_ms) {
            return Err("notifications.timeout_ms must be between 1000 and 30000".into());
        }
        Ok(())
    }
}
