use crate::config::InactiveDimSettings;
use ferese_layout::WindowId;
use std::time::Duration;

pub(crate) fn target(
    settings: InactiveDimSettings,
    focused: Option<WindowId>,
    window: WindowId,
    overview: bool,
) -> f64 {
    if settings.enabled && !overview && focused.is_some() && focused != Some(window) {
        settings.amount
    } else {
        0.0
    }
}

/// A bounded, interruption-safe opacity transition; it never overshoots.
#[derive(Debug)]
pub(crate) struct DimAnimation {
    pub current: f64,
    start: f64,
    target: f64,
    elapsed: f64,
}

impl DimAnimation {
    pub fn new(value: f64) -> Self {
        Self {
            current: value,
            start: value,
            target: value,
            elapsed: 0.0,
        }
    }

    pub fn advance(&mut self, target: f64, delta: Duration, duration_ms: f64) -> bool {
        if self.target != target {
            self.start = self.current;
            self.target = target;
            self.elapsed = 0.0;
        }
        if duration_ms == 0.0 {
            self.current = target;
            return false;
        }
        self.elapsed += delta.as_secs_f64() * 1000.0;
        let progress = (self.elapsed / duration_ms).min(1.0);
        if progress == 1.0 {
            self.current = target;
            return false;
        }
        let eased = progress * progress * (3.0 - 2.0 * progress);
        self.current = self.start + (target - self.start) * eased;
        self.current != target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unfocused_windows_dim_and_overview_disables_it() {
        let settings = InactiveDimSettings {
            enabled: true,
            amount: 0.15,
            duration_ms: 150.0,
        };
        let left = WindowId(1);
        let right = WindowId(2);
        assert_eq!(target(settings, Some(right), left, false), 0.15);
        assert_eq!(target(settings, Some(right), right, false), 0.0);
        assert_eq!(target(settings, Some(right), left, true), 0.0);
        assert_eq!(target(settings, None, left, false), 0.0);
        assert_eq!(
            target(
                InactiveDimSettings {
                    enabled: false,
                    ..settings
                },
                Some(right),
                left,
                false
            ),
            0.0
        );
        assert_eq!(
            target(
                InactiveDimSettings {
                    amount: 0.0,
                    ..settings
                },
                Some(right),
                left,
                false
            ),
            0.0
        );
    }

    #[test]
    fn focus_transition_is_smooth_bounded_and_settles() {
        let mut dim = DimAnimation::new(0.0);
        assert!(dim.advance(0.15, Duration::from_millis(75), 150.0));
        assert!((dim.current - 0.075).abs() < 1e-9);
        assert!(!dim.advance(0.15, Duration::from_millis(75), 150.0));
        assert_eq!(dim.current, 0.15);
        assert!(!dim.advance(0.15, Duration::from_secs(10), 150.0));
    }

    #[test]
    fn reversing_focus_starts_from_current_opacity() {
        let mut dim = DimAnimation::new(0.0);
        dim.advance(0.15, Duration::from_millis(75), 150.0);
        let before = dim.current;
        assert!(dim.advance(0.0, Duration::ZERO, 150.0));
        assert_eq!(dim.current, before);
        assert!(!dim.advance(0.0, Duration::from_millis(150), 150.0));
        assert_eq!(dim.current, 0.0);
    }

    #[test]
    fn zero_duration_snaps_for_reduced_motion() {
        let mut dim = DimAnimation::new(0.15);
        assert!(!dim.advance(0.0, Duration::ZERO, 0.0));
        assert_eq!(dim.current, 0.0);
    }
}
