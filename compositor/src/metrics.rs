use std::time::{Duration, Instant};

use smithay::utils::{Physical, Rectangle};

const REPORT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct FrameEffectMetrics;

#[derive(Debug)]
pub(crate) struct RenderMetrics {
    enabled: bool,
    output: String,
    interval_started: Instant,
    rendered_frames: u64,
    damaged_pixels: u64,
    total_render_time: Duration,
    longest_render_time: Duration,
    frame_times: Vec<Duration>,
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
            frame_times: Vec::new(),
        }
    }

    pub(crate) fn record_frame(
        &mut self,
        render_time: Duration,
        damage: &[Rectangle<i32, Physical>],
        missed_deadlines: u64,
        _effects: FrameEffectMetrics,
    ) {
        if !self.enabled {
            return;
        }

        self.rendered_frames += 1;
        self.damaged_pixels += damage.iter().map(rectangle_area).sum::<u64>();
        self.total_render_time += render_time;
        self.longest_render_time = self.longest_render_time.max(render_time);
        if self.frame_times.len() == 512 {
            self.frame_times.remove(0);
        }
        self.frame_times.push(render_time);

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
        self.frame_times.sort_unstable();
        let percentile = |percent: usize| {
            let index = (self.frame_times.len() * percent)
                .div_ceil(100)
                .saturating_sub(1);
            self.frame_times
                .get(index)
                .copied()
                .unwrap_or_default()
                .as_micros()
        };
        tracing::info!(
            target: "ferese::render",
            output = %self.output,
            interval_ms = elapsed.as_millis(),
            frames = self.rendered_frames,
            damaged_pixels = self.damaged_pixels,
            average_render_us,
            longest_render_us = self.longest_render_time.as_micros(),
            p95_render_us = percentile(95),
            p99_render_us = percentile(99),
            missed_deadlines,
            "render performance"
        );

        self.interval_started = now;
        self.rendered_frames = 0;
        self.damaged_pixels = 0;
        self.total_render_time = Duration::ZERO;
        self.longest_render_time = Duration::ZERO;
        self.frame_times.clear();
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
            FrameEffectMetrics::default(),
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
            FrameEffectMetrics,
        );

        assert_eq!(metrics.rendered_frames, 1);
        assert_eq!(metrics.damaged_pixels, 5_200);
        assert_eq!(metrics.total_render_time, Duration::from_millis(2));
        assert_eq!(metrics.longest_render_time, Duration::from_millis(2));
        assert_eq!(metrics.frame_times, vec![Duration::from_millis(2)]);
    }

    #[test]
    fn profiling_samples_are_bounded() {
        let mut metrics = RenderMetrics::new("test", true, Instant::now());
        for _ in 0..2000 {
            metrics.record_frame(Duration::from_millis(1), &[], 0, FrameEffectMetrics);
        }
        assert_eq!(metrics.frame_times.len(), 512);
    }
}
