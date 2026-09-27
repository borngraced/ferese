//! A presentation barrier, not an event-loop or client-commit blocker.
//! Grouping barriers by workspace keeps viewport and neighboring tiles on the
//! same clock. A serial identifies readiness even for cell-grid clients whose
//! committed size is smaller than the configure, and after rapid reversals.
use std::time::Duration;

use smithay::utils::{Logical, Rectangle, Serial};

const DEADLINE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug)]
pub(crate) struct ResizeTransaction {
    serial: Serial,
    started: Duration,
    source_geometry: Option<Rectangle<i32, Logical>>,
}

impl ResizeTransaction {
    pub(crate) fn new(serial: Serial, started: Duration) -> Self {
        Self {
            serial,
            started,
            source_geometry: None,
        }
    }

    pub(crate) fn with_source_geometry(mut self, geometry: Rectangle<i32, Logical>) -> Self {
        self.source_geometry = Some(geometry);
        self
    }

    pub(crate) fn source_geometry(self) -> Option<Rectangle<i32, Logical>> {
        self.source_geometry
    }

    pub(crate) fn accepts(self, committed: Option<Serial>) -> bool {
        committed.is_some_and(|serial| serial >= self.serial)
    }

    pub(crate) fn expired(self, now: Duration) -> bool {
        now.saturating_sub(self.started) >= DEADLINE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledgement_alone_or_stale_commit_does_not_release_resize() {
        let transaction = ResizeTransaction::new(12.into(), Duration::ZERO);
        assert!(!transaction.accepts(None));
        assert!(!transaction.accepts(Some(11.into())));
        assert!(transaction.accepts(Some(12.into())));
        assert!(transaction.accepts(Some(13.into())));
    }

    #[test]
    fn serial_comparison_handles_wraparound() {
        let transaction = ResizeTransaction::new(u32::MAX.into(), Duration::ZERO);
        assert!(transaction.accepts(Some(0.into())));
        assert!(!transaction.accepts(Some((u32::MAX - 1).into())));
    }

    #[test]
    fn unresponsive_client_has_bounded_wait() {
        let transaction = ResizeTransaction::new(1.into(), Duration::from_millis(10));
        assert!(!transaction.expired(Duration::from_millis(309)));
        assert!(transaction.expired(Duration::from_millis(310)));
    }

    #[test]
    fn shrinking_commit_retains_the_previous_full_frame_geometry() {
        let source = Rectangle::new((4, 8).into(), (1200, 800).into());
        let transaction =
            ResizeTransaction::new(1.into(), Duration::ZERO).with_source_geometry(source);
        assert_eq!(transaction.source_geometry(), Some(source));
    }
}
