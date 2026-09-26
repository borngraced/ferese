//! Filesystem events only: idle timer ticks never read/stat the config.
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    io::Read,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

fn read_source(path: &std::path::Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut source = String::new();
    file.take(60 * 1024 + 1)
        .read_to_string(&mut source)
        .map_err(|e| e.to_string())?;
    if source.len() > 60 * 1024 {
        return Err("configuration exceeds the 60 KiB live-reload limit".into());
    }
    Ok(source)
}

pub(crate) struct ConfigMonitor {
    _watcher: RecommendedWatcher,
    events: mpsc::Receiver<()>,
    path: PathBuf,
    dirty: Option<Instant>,
    observed: Option<String>,
}
impl ConfigMonitor {
    pub(crate) fn new(path: PathBuf) -> notify::Result<Self> {
        let (sender, events) = mpsc::sync_channel(1);
        let target = path.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event
                    && !event.kind.is_access()
                    && event
                        .paths
                        .iter()
                        .any(|p| p == &target || target.starts_with(p))
                {
                    let _ = sender.try_send(());
                }
            })?;
        // Watch the directory, not the inode: atomic editor saves replace it.
        let mut directory = path.parent().unwrap_or(std::path::Path::new("."));
        while !directory.is_dir() {
            directory = directory.parent().unwrap_or(std::path::Path::new("."));
        }
        watcher.watch(
            directory,
            if Some(directory) == path.parent() {
                RecursiveMode::NonRecursive
            } else {
                RecursiveMode::Recursive
            },
        )?;
        let initial_dirty = path.is_file().then(Instant::now);
        Ok(Self {
            _watcher: watcher,
            events,
            path,
            dirty: initial_dirty,
            observed: None,
        })
    }
    pub(crate) fn poll(&mut self, now: Instant) -> Option<Result<String, String>> {
        if self.events.try_iter().next().is_some() {
            self.dirty = Some(now);
        }
        if self
            .dirty
            .is_none_or(|since| now.saturating_duration_since(since) < Duration::from_millis(120))
        {
            return None;
        }
        self.dirty = None;
        let source = match read_source(&self.path) {
            Ok(source) => source,
            Err(error) => {
                return Some(Err(format!(
                    "{}: {error}; retaining active config",
                    self.path.display()
                )));
            }
        };
        if self.observed.as_ref() == Some(&source) {
            return None;
        }
        self.observed = Some(source.clone());
        Some(Ok(source))
    }
}

impl crate::Ferese {
    pub(crate) fn reload_config_source(&mut self, source: String) -> Result<(), String> {
        if source.len() > 60 * 1024 {
            return Err("configuration exceeds the 60 KiB live-reload limit".into());
        }
        if self.config_source.as_ref() == Some(&source) {
            return Ok(());
        }
        // Validate every runtime field before mutating any scene/input state.
        let config = crate::config::Config::parse_source(&source).map_err(|e| e.to_string())?;
        let runtime = config.runtime_config().map_err(|e| e.to_string())?;
        self.apply_runtime_config(runtime)?;
        self.config_source = Some(source.clone());
        self.shell_resources.retain(|resource| {
            if let Ok(shell) = resource.upgrade() {
                if smithay::reexports::wayland_server::Resource::version(&shell) >= 2 {
                    crate::shell_control::send_shell_config(&shell, &source);
                }
                true
            } else {
                false
            }
        });
        tracing::info!("configuration reloaded live");
        Ok(())
    }
    pub(crate) fn reload_config(&mut self) -> Result<(), String> {
        let path = crate::config::config_path().ok_or("config directory is unavailable")?;
        let source = read_source(&path)?;
        self.reload_config_source(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "ferese-reload-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn runtime_validation_rejects_bad_edits_before_application() {
        for source in [
            "[animations]\nspeed = 0",
            "[theme.focus_ring.gradient]\nfrom = 'bad'\nto = '#ffffff'",
            "[status]\nlow_battery_threshold = 'bad'",
            "[layout]\ninner_gap = -1",
        ] {
            assert!(
                crate::config::Config::parse_source(source)
                    .and_then(|config| config.runtime_config())
                    .is_err()
            );
        }
        assert!(
            crate::config::Config::parse_source("[animations]\nspeed = 0.75")
                .unwrap()
                .runtime_config()
                .is_ok()
        );
    }
    #[test]
    fn directory_watch_detects_atomic_save_and_does_not_read_during_idle() {
        let directory = Directory::new();
        let path = directory.0.join("config.toml");
        std::fs::write(&path, "[animations]\nspeed = 1").unwrap();
        let mut monitor = ConfigMonitor::new(path.clone()).unwrap();
        monitor.dirty = Some(Instant::now() - Duration::from_secs(1));
        assert!(monitor.poll(Instant::now()).unwrap().is_ok());
        // With no event, even a deliberately missing path is never read.
        let original_path = monitor.path.clone();
        monitor.path = directory.0.join("missing");
        assert!(monitor.poll(Instant::now()).is_none());
        monitor.path = original_path;
        let temporary = directory.0.join("config.toml.new");
        std::fs::write(&temporary, "[animations]\nspeed = 0.75").unwrap();
        std::fs::rename(&temporary, &path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = monitor.poll(Instant::now()) {
                assert_eq!(result.unwrap(), "[animations]\nspeed = 0.75");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "atomic save notification never settled"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn source_size_limit_is_bounded_and_debounce_coalesces_edits() {
        let directory = Directory::new();
        let path = directory.0.join("config.toml");
        std::fs::write(&path, "x".repeat(60 * 1024 + 1)).unwrap();
        assert!(read_source(&path).is_err());
        std::fs::write(&path, "[animations]\nspeed = 1").unwrap();
        let mut monitor = ConfigMonitor::new(path).unwrap();
        let now = Instant::now();
        monitor.dirty = Some(now);
        assert!(monitor.poll(now + Duration::from_millis(119)).is_none());
        assert!(
            monitor
                .poll(now + Duration::from_millis(121))
                .unwrap()
                .is_ok()
        );
    }
}
