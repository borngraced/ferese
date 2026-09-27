//! A swipe commits once on release; cancelled and ambiguous gestures do nothing.

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u32)]
pub(crate) enum SwipeDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Default)]
pub(crate) struct Swipe {
    delta: Option<(f64, f64)>,
    threshold: f64,
    fingers: u32,
}

impl Swipe {
    pub fn begin(&mut self, fingers: u32, blocked: bool, threshold: u16) {
        self.delta = ((3..=5).contains(&fingers) && !blocked).then_some((0.0, 0.0));
        self.threshold = f64::from(threshold);
        self.fingers = fingers;
    }

    pub fn fingers(&self) -> u32 {
        self.fingers
    }

    pub fn active(&self) -> bool {
        self.delta.is_some()
    }

    pub fn update(&mut self, dx: f64, dy: f64) {
        if let Some((x, y)) = &mut self.delta {
            *x += dx;
            *y += dy;
        }
    }

    pub fn finish(&mut self, cancelled: bool) -> Option<SwipeDirection> {
        let (x, y) = self.delta.take()?;
        if cancelled || !x.is_finite() || !y.is_finite() {
            return None;
        }
        const DOMINANCE: f64 = 1.25;
        if x.abs() >= self.threshold && x.abs() > y.abs() * DOMINANCE {
            Some(if x < 0.0 {
                SwipeDirection::Left
            } else {
                SwipeDirection::Right
            })
        } else if y.abs() >= self.threshold && y.abs() > x.abs() * DOMINANCE {
            Some(if y < 0.0 {
                SwipeDirection::Up
            } else {
                SwipeDirection::Down
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swipe_accumulates_motion_and_commits_only_once_on_release() {
        let mut swipe = Swipe::default();
        swipe.begin(3, false, 80);
        for _ in 0..10 {
            swipe.update(-10.0, 1.0);
        }
        assert!(swipe.active());
        assert_eq!(swipe.finish(false), Some(SwipeDirection::Left));
        assert!(!swipe.active());
        assert_eq!(swipe.finish(false), None);
    }

    #[test]
    fn vertical_and_horizontal_swipes_are_distinct() {
        for (delta, expected) in [
            ((0.0, -100.0), SwipeDirection::Up),
            ((0.0, 100.0), SwipeDirection::Down),
            ((-100.0, 0.0), SwipeDirection::Left),
            ((100.0, 0.0), SwipeDirection::Right),
        ] {
            let mut swipe = Swipe::default();
            swipe.begin(3, false, 80);
            swipe.update(delta.0, delta.1);
            assert_eq!(swipe.finish(false), Some(expected));
        }
    }

    #[test]
    fn cancellation_jitter_diagonals_and_nonfinite_motion_do_not_navigate() {
        for (x, y, cancelled) in [
            (200.0, 0.0, true),
            (79.0, 0.0, false),
            (100.0, 100.0, false),
            (f64::NAN, 100.0, false),
        ] {
            let mut swipe = Swipe::default();
            swipe.begin(3, false, 80);
            swipe.update(x, y);
            assert_eq!(swipe.finish(cancelled), None);
            assert!(!swipe.active());
        }
    }

    #[test]
    fn other_finger_counts_and_blocked_gestures_are_not_intercepted() {
        for (fingers, blocked) in [(2, false), (6, false), (3, true)] {
            let mut swipe = Swipe::default();
            swipe.begin(fingers, blocked, 80);
            assert!(!swipe.active());
            swipe.update(200.0, 0.0);
            assert_eq!(swipe.finish(false), None);
        }
    }

    #[test]
    fn configured_distance_changes_when_a_swipe_commits() {
        let mut swipe = Swipe::default();
        swipe.begin(3, false, 120);
        swipe.update(100.0, 0.0);
        assert_eq!(swipe.finish(false), None);
        swipe.begin(3, false, 40);
        swipe.update(100.0, 0.0);
        assert_eq!(swipe.finish(false), Some(SwipeDirection::Right));
    }
}
