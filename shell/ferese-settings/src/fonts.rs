use std::process::Command;
use std::sync::OnceLock;

pub fn families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();

    FAMILIES.get_or_init(|| {
        let output = Command::new("fc-list")
            .args(["--format", "%{family}\\n"])
            .output()
            .ok()
            .filter(|output| output.status.success());
        let mut families: Vec<String> = output
            .as_ref()
            .map(|output| String::from_utf8_lossy(&output.stdout))
            .unwrap_or_default()
            .lines()
            .flat_map(|line| line.split(','))
            .map(str::trim)
            .filter(|family| !family.is_empty() && family.len() <= 128)
            .map(str::to_owned)
            .collect();

        families.extend([
            "Comfortaa".into(),
            "sans-serif".into(),
            "serif".into(),
            "monospace".into(),
        ]);
        families.sort_unstable_by_key(|family| family.to_lowercase());
        families.dedup();
        families.insert(0, "Default font".into());
        families
    })
}
