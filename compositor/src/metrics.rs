use std::time::{Duration, Instant};

use smithay::utils::{Physical, Rectangle};

const REPORT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub(crate) struct RenderMetrics {
    enabled: bool,
    output: String,
    interval_started: Instant,
    rendered_frames: u64,
    damaged_pixels: u64,
    total_render_time: Duration,
    longest_render_time: Duration,
}

impl RenderMetrics {
    pub(crate) fn from_environment(output: impl Into<String>) -> Self {
        Self::new(
            output,
            std::env::var_os("FERESE_TRACE_PERFORMANCE").is_some(),
            Instant::now(),
        )
    }

    fn new(output: impl Into<String>, enabled: bool, now: Instant) -> Self {
        Self {
            enabled,
            output: output.into(),
            interval_started: now,
            rendered_frames: 0,
            damaged_pixels: 0,
            total_render_time: Duration::ZERO,
            longest_render_time: Duration::ZERO,
        }
    }

    pub(crate) fn record_frame(
        &mut self,
        render_time: Duration,
        damage: &[Rectangle<i32, Physical>],
        missed_deadlines: u64,
    ) {
        if !self.enabled {
            return;
        }

        self.rendered_frames += 1;
        self.damaged_pixels += damage.iter().map(rectangle_area).sum::<u64>();
        self.total_render_time += render_time;
        self.longest_render_time = self.longest_render_time.max(render_time);

        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.interval_started);
        if elapsed < REPORT_INTERVAL {
            return;
        }

        let average_render_us = self
            .total_render_time
            .as_micros()
            .checked_div(u128::from(self.rendered_frames))
            .unwrap_or(0);
        tracing::info!(
            target: "ferese::render",
            output = %self.output,
            interval_ms = elapsed.as_millis(),
            frames = self.rendered_frames,
            damaged_pixels = self.damaged_pixels,
            average_render_us,
            longest_render_us = self.longest_render_time.as_micros(),
            missed_deadlines,
            "render performance"
        );

        self.interval_started = now;
        self.rendered_frames = 0;
        self.damaged_pixels = 0;
        self.total_render_time = Duration::ZERO;
        self.longest_render_time = Duration::ZERO;
    }
}

fn rectangle_area(rectangle: &Rectangle<i32, Physical>) -> u64 {
    u64::try_from(rectangle.size.w.max(0)).unwrap_or(0)
        * u64::try_from(rectangle.size.h.max(0)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_metrics_do_not_accumulate() {
        let mut metrics = RenderMetrics::new("test", false, Instant::now());

        metrics.record_frame(
            Duration::from_millis(2),
            &[Rectangle::new((0, 0).into(), (100, 50).into())],
            0,
        );

        assert_eq!(metrics.rendered_frames, 0);
        assert_eq!(metrics.damaged_pixels, 0);
    }

    #[test]
    fn enabled_metrics_count_frames_pixels_and_render_time() {
        let mut metrics = RenderMetrics::new("test", true, Instant::now());

        metrics.record_frame(
            Duration::from_millis(2),
            &[
                Rectangle::new((0, 0).into(), (100, 50).into()),
                Rectangle::new((10, 10).into(), (20, 10).into()),
            ],
            0,
        );

        assert_eq!(metrics.rendered_frames, 1);
        assert_eq!(metrics.damaged_pixels, 5_200);
        assert_eq!(metrics.total_render_time, Duration::from_millis(2));
        assert_eq!(metrics.longest_render_time, Duration::from_millis(2));
    }
}
