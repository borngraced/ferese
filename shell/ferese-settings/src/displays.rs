use serde_json::{Value, json};

use crate::store::{Edit, set};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
}

impl Mode {
    fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            width: value["width"].as_u64()?.try_into().ok()?,
            height: value["height"].as_u64()?.try_into().ok()?,
            refresh: value["refresh_millihertz"].as_u64()?.try_into().ok()?,
        })
    }

    pub fn config(self) -> String {
        format!("{}x{}@{:.3}", self.width, self.height, f64::from(self.refresh) / 1000.)
    }

    pub fn label(self) -> String {
        if self.refresh % 1000 == 0 {
            format!("{} Hz", self.refresh / 1000)
        } else {
            format!("{:.3} Hz", f64::from(self.refresh) / 1000.)
        }
    }
}

#[derive(Clone, Debug)]
pub struct Display {
    pub connector: String,
    pub identity: String,
    pub profile: Option<String>,
    pub current: Option<Mode>,
    pub modes: Vec<Mode>,
}

pub fn load() -> Result<Vec<Display>, String> {
    let mut connection = ferese_ipc::theme::Connection::connect().map_err(|error| error.to_string())?;
    let value = connection.call("get-outputs", json!({}))?;
    let values = value.as_array().ok_or("Invalid display response")?;
    Ok(values
        .iter()
        .filter_map(|value| {
            Some(Display {
                connector: value["connector"].as_str()?.to_owned(),
                identity: value["identity"].as_str().unwrap_or("").to_owned(),
                profile: value["profile"].as_str().map(str::to_owned),
                current: Mode::from_value(&value["current_mode"]),
                modes: value["available_modes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Mode::from_value)
                    .collect(),
            })
        })
        .collect())
}

pub fn choices(display: &Display, configured: &str) -> Vec<Mode> {
    let size = configured
        .split('@')
        .next()
        .and_then(|size| size.split_once('x'))
        .and_then(|(width, height)| Some((width.parse::<u32>().ok()?, height.parse::<u32>().ok()?)))
        .or_else(|| display.current.map(|mode| (mode.width, mode.height)));
    let mut modes = display
        .modes
        .iter()
        .copied()
        .filter(|mode| Some((mode.width, mode.height)) == size && mode.refresh > 0)
        .collect::<Vec<_>>();
    modes.sort_by_key(|mode| mode.refresh);
    modes.dedup();
    modes
}

pub fn edits(prefix: &str, mode: Mode, automatic: bool) -> Vec<Edit> {
    vec![
        set(&format!("{prefix}.mode"), mode.config()),
        set(&format!("{prefix}.auto_refresh"), automatic),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Snapshot;

    #[test]
    fn refresh_choices_preserve_resolution_and_fractional_rates() {
        let display = Display {
            connector: "eDP-1".into(),
            identity: "panel".into(),
            profile: None,
            current: Some(Mode {
                width: 2880,
                height: 1800,
                refresh: 120000,
            }),
            modes: vec![
                Mode {
                    width: 2880,
                    height: 1800,
                    refresh: 120000,
                },
                Mode {
                    width: 2880,
                    height: 1800,
                    refresh: 59940,
                },
                Mode {
                    width: 1920,
                    height: 1080,
                    refresh: 60000,
                },
                Mode {
                    width: 2880,
                    height: 1800,
                    refresh: 59940,
                },
            ],
        };
        let choices = choices(&display, "2880x1800@120");
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].label(), "59.940 Hz");
        assert_eq!(choices[0].config(), "2880x1800@59.940");
        assert_eq!(choices[1].label(), "120 Hz");
    }

    #[test]
    fn manual_choice_disables_auto_and_auto_restores_high_rate() {
        let mut snapshot = Snapshot::parse(
            "output-profile laptop { output eDP-1 mode=\"2880x1800@120\" scale=1.75 auto-refresh=#true; }".into(),
        )
        .unwrap();
        let prefix = "output_profiles.0.outputs.0";
        for edit in edits(
            prefix,
            Mode {
                width: 2880,
                height: 1800,
                refresh: 60000,
            },
            false,
        ) {
            snapshot.edit(&edit).unwrap();
        }
        assert!(!snapshot.boolean(&format!("{prefix}.auto_refresh"), true));
        assert_eq!(snapshot.string(&format!("{prefix}.mode"), ""), "2880x1800@60.000");
        assert_eq!(snapshot.number(&format!("{prefix}.scale"), 1.), 1.75);
        for edit in edits(
            prefix,
            Mode {
                width: 2880,
                height: 1800,
                refresh: 120000,
            },
            true,
        ) {
            snapshot.edit(&edit).unwrap();
        }
        assert!(snapshot.boolean(&format!("{prefix}.auto_refresh"), false));
        assert_eq!(snapshot.string(&format!("{prefix}.mode"), ""), "2880x1800@120.000");
    }
}
