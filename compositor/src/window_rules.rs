use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WindowRuleConfig {
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub transient: Option<bool>,
    pub workspace: Option<u32>,
    pub floating: Option<bool>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub fullscreen: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowRule {
    app_id: Option<String>,
    title: Option<String>,
    transient: Option<bool>,
    workspace: Option<u32>,
    floating: Option<bool>,
    width: Option<f64>,
    height: Option<f64>,
    fullscreen: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowRuleResult {
    pub workspace: Option<u32>,
    pub floating: Option<bool>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub fullscreen: Option<bool>,
}

pub fn validate(configured: &[WindowRuleConfig]) -> Result<Vec<WindowRule>, String> {
    configured
        .iter()
        .enumerate()
        .map(|(index, rule)| validate_rule(index, rule))
        .collect()
}

pub fn resolve(
    rules: &[WindowRule],
    app_id: Option<&str>,
    title: Option<&str>,
    transient: bool,
) -> WindowRuleResult {
    let app_id = app_id.map(normalize_app_id);
    let mut result = WindowRuleResult::default();

    for rule in rules {
        let app_id_matches = rule
            .app_id
            .as_deref()
            .is_none_or(|expected| app_id.as_deref() == Some(expected));
        let title_matches = rule
            .title
            .as_deref()
            .is_none_or(|expected| title == Some(expected));
        let transient_matches = rule.transient.is_none_or(|expected| expected == transient);
        if !app_id_matches || !title_matches || !transient_matches {
            continue;
        }

        result.workspace = rule.workspace.or(result.workspace);
        result.floating = rule.floating.or(result.floating);
        result.width = rule.width.or(result.width);
        result.height = rule.height.or(result.height);
        result.fullscreen = rule.fullscreen.or(result.fullscreen);
    }

    result
}

fn validate_rule(index: usize, rule: &WindowRuleConfig) -> Result<WindowRule, String> {
    if rule.app_id.is_none() && rule.title.is_none() && rule.transient.is_none() {
        return Err(format!(
            "window_rules[{index}] must match app_id, title, or transient"
        ));
    }
    if rule.workspace == Some(0) {
        return Err(format!(
            "window_rules[{index}].workspace must be greater than zero"
        ));
    }

    let app_id = rule
        .app_id
        .as_deref()
        .map(nonempty_matcher)
        .transpose()
        .map_err(|message| format!("window_rules[{index}].app_id {message}"))?
        .map(normalize_app_id);
    let title = rule
        .title
        .as_deref()
        .map(nonempty_matcher)
        .transpose()
        .map_err(|message| format!("window_rules[{index}].title {message}"))?
        .map(str::to_owned);
    let width = positive_dimension(rule.width, index, "width")?;
    let height = positive_dimension(rule.height, index, "height")?;

    Ok(WindowRule {
        app_id,
        title,
        transient: rule.transient,
        workspace: rule.workspace,
        floating: rule.floating,
        width,
        height,
        fullscreen: rule.fullscreen,
    })
}

fn nonempty_matcher(value: &str) -> Result<&str, &'static str> {
    let value = value.trim();
    if value.is_empty() {
        Err("cannot be empty")
    } else {
        Ok(value)
    }
}

fn positive_dimension(
    value: Option<f64>,
    index: usize,
    field: &'static str,
) -> Result<Option<f64>, String> {
    if value.is_none_or(|value| value.is_finite() && value > 0.0) {
        Ok(value)
    } else {
        Err(format!(
            "window_rules[{index}].{field} must be a positive finite number"
        ))
    }
}

fn normalize_app_id(value: &str) -> String {
    value
        .trim()
        .strip_suffix(".desktop")
        .unwrap_or(value.trim())
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(app_id: &str) -> WindowRuleConfig {
        WindowRuleConfig {
            app_id: Some(app_id.to_owned()),
            title: None,
            transient: None,
            workspace: None,
            floating: None,
            width: None,
            height: None,
            fullscreen: None,
        }
    }

    #[test]
    fn app_ids_are_normalized_for_matching() {
        let rules = validate(&[WindowRuleConfig {
            floating: Some(true),
            ..config("Org.Example.Editor.desktop")
        }])
        .unwrap();

        assert_eq!(
            resolve(&rules, Some("org.example.editor"), None, false).floating,
            Some(true)
        );
    }

    #[test]
    fn later_matching_rules_override_only_their_fields() {
        let rules = validate(&[
            WindowRuleConfig {
                workspace: Some(3),
                floating: Some(true),
                width: Some(800.0),
                ..config("editor")
            },
            WindowRuleConfig {
                floating: Some(false),
                fullscreen: Some(true),
                ..config("editor")
            },
        ])
        .unwrap();

        assert_eq!(
            resolve(&rules, Some("editor"), None, false),
            WindowRuleResult {
                workspace: Some(3),
                floating: Some(false),
                width: Some(800.0),
                fullscreen: Some(true),
                ..WindowRuleResult::default()
            }
        );
    }

    #[test]
    fn rejects_catch_all_and_invalid_dimensions() {
        let catch_all = WindowRuleConfig {
            app_id: None,
            title: None,
            transient: None,
            workspace: None,
            floating: Some(true),
            width: None,
            height: None,
            fullscreen: None,
        };
        let invalid_size = WindowRuleConfig {
            width: Some(f64::NAN),
            ..config("editor")
        };

        assert!(validate(&[catch_all]).is_err());
        assert!(validate(&[invalid_size]).is_err());
    }
}
