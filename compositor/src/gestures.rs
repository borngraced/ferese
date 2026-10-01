//! Navigation previews follow finger motion; actions commit once on release.

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
    direction: Option<SwipeDirection>,
    preview_started: bool,
}

impl Swipe {
    pub fn begin(&mut self, fingers: u32, blocked: bool, threshold: u16) {
        self.delta = ((3..=5).contains(&fingers) && !blocked).then_some((0.0, 0.0));
        self.threshold = f64::from(threshold);
        self.fingers = fingers;
        self.direction = None;
        self.preview_started = false;
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
            if self.direction.is_none() {
                self.direction = dominant_direction(*x, *y, 6.0);
            }
        }
    }

    pub fn mark_preview_started(&mut self) {
        self.preview_started = true;
    }

    pub fn preview_started(&self) -> bool {
        self.preview_started
    }

    pub fn preview(&self) -> Option<(SwipeDirection, f64)> {
        let (x, y) = self.delta?;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let direction = self.direction?;
        let distance = match direction {
            SwipeDirection::Left => -x,
            SwipeDirection::Right => x,
            SwipeDirection::Up => -y,
            SwipeDirection::Down => y,
        };
        Some((direction, (distance / (self.threshold.max(1.0) * 2.0)).clamp(0.0, 1.0)))
    }

    pub fn finish(&mut self, cancelled: bool) -> Option<SwipeDirection> {
        let (x, y) = self.delta.take()?;
        if cancelled {
            return None;
        }
        dominant_direction(x, y, self.threshold).filter(|direction| Some(*direction) == self.direction)
    }
}

fn dominant_direction(x: f64, y: f64, threshold: f64) -> Option<SwipeDirection> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    const DOMINANCE: f64 = 1.25;
    if x.abs() >= threshold && x.abs() > y.abs() * DOMINANCE {
        Some(if x < 0.0 {
            SwipeDirection::Left
        } else {
            SwipeDirection::Right
        })
    } else if y.abs() >= threshold && y.abs() > x.abs() * DOMINANCE {
        Some(if y < 0.0 {
            SwipeDirection::Up
        } else {
            SwipeDirection::Down
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_and_new_gestures_reset_preview_state() {
        let mut swipe = Swipe::default();
        swipe.begin(3, false, 80);
        swipe.update(0.0, -200.0);
        swipe.mark_preview_started();
        assert_eq!(swipe.preview(), Some((SwipeDirection::Up, 1.0)));
        assert_eq!(swipe.finish(true), None);
        assert!(
            swipe.preview_started(),
            "release must suppress a second action after a preview"
        );
        assert_eq!(swipe.preview(), None);
        swipe.begin(4, false, 80);
        assert!(!swipe.preview_started());
        swipe.update(-100.0, 0.0);
        assert_eq!(swipe.finish(false), Some(SwipeDirection::Left));
        swipe.begin(3, true, 80);
        swipe.update(0.0, -200.0);
        assert!(!swipe.active());
        assert_eq!(swipe.finish(false), None);
    }

    #[test]
    fn preview_tracks_motion_and_reversal_before_release() {
        let mut swipe = Swipe::default();
        swipe.begin(3, false, 80);
        swipe.update(0.0, -40.0);
        assert_eq!(swipe.preview(), Some((SwipeDirection::Up, 0.25)));
        swipe.update(0.0, -40.0);
        assert_eq!(swipe.preview(), Some((SwipeDirection::Up, 0.5)));
        swipe.update(0.0, 60.0);
        assert_eq!(swipe.preview(), Some((SwipeDirection::Up, 0.125)));
        assert_eq!(swipe.finish(false), None);
        assert_eq!(swipe.preview(), None);
    }

    #[test]
    fn reversal_across_the_origin_cancels_instead_of_committing_another_axis() {
        let mut swipe = Swipe::default();
        swipe.begin(3, false, 80);
        swipe.update(0.0, -40.0);
        swipe.update(0.0, 160.0);
        assert_eq!(swipe.preview(), Some((SwipeDirection::Up, 0.0)));
        assert_eq!(swipe.finish(false), None);
    }

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
