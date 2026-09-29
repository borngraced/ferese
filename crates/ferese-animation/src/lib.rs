use std::time::Duration;

use ferese_layout::Rect;

const MAX_FRAME_DELTA: Duration = Duration::from_millis(100);
const MAX_STEP_SECONDS: f64 = 1.0 / 240.0;
const CLIENT_COMMIT_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringConfig {
    pub mass: f64,
    pub stiffness: f64,
    pub damping: f64,
    pub position_tolerance: f64,
    pub velocity_tolerance: f64,
}

impl Default for SpringConfig {
    fn default() -> Self {
        Self {
            mass: 1.0,
            stiffness: 700.0,
            damping: 53.0,
            position_tolerance: 0.1,
            velocity_tolerance: 0.1,
        }
    }
}

impl SpringConfig {
    fn normalized(self) -> Self {
        let defaults = Self::default();

        Self {
            mass: positive_or(self.mass, defaults.mass),
            stiffness: positive_or(self.stiffness, defaults.stiffness),
            damping: non_negative_or(self.damping, defaults.damping),
            position_tolerance: positive_or(self.position_tolerance, defaults.position_tolerance),
            velocity_tolerance: positive_or(self.velocity_tolerance, defaults.velocity_tolerance),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectVelocity {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimatedRect {
    pub current: Rect,
    pub target: Rect,
    pub velocity: RectVelocity,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimatedValue {
    pub current: f64,
    pub target: f64,
    pub velocity: f64,
}

impl AnimatedValue {
    pub fn new(value: f64) -> Self {
        let value = finite_or_zero(value);

        Self {
            current: value,
            target: value,
            velocity: 0.0,
        }
    }

    pub fn set_target(&mut self, target: f64) {
        self.target = finite_or_zero(target);
    }

    pub fn retarget_preserving_motion(&mut self, target: f64) {
        let target = finite_or_zero(target);
        let direction = (target - self.current).signum();

        if direction != 0.0 && self.velocity.signum() != direction {
            self.velocity *= 0.35;
        }
        self.target = target;
    }

    pub fn snap(&mut self) {
        self.current = self.target;
        self.velocity = 0.0;
    }

    pub fn advance(&mut self, delta: Duration, config: SpringConfig) -> bool {
        let config = config.normalized();
        if (self.current - self.target).abs() <= config.position_tolerance
            && self.velocity.abs() <= config.velocity_tolerance
        {
            self.snap();
            return false;
        }

        let seconds = delta.min(MAX_FRAME_DELTA).as_secs_f64();
        let steps = (seconds / MAX_STEP_SECONDS).ceil().max(1.0) as usize;
        let step = seconds / steps as f64;

        for _ in 0..steps {
            let previous_error = self.current - self.target;
            advance_value(
                &mut self.current,
                self.target,
                &mut self.velocity,
                step,
                config,
            );
            let current_error = self.current - self.target;
            if previous_error != 0.0 && previous_error.signum() != current_error.signum() {
                self.snap();
                break;
            }
        }

        if (self.current - self.target).abs() <= config.position_tolerance
            && self.velocity.abs() <= config.velocity_tolerance
        {
            self.snap();
            false
        } else {
            true
        }
    }
}

impl AnimatedRect {
    pub fn new(rect: Rect) -> Self {
        Self {
            current: rect,
            target: rect,
            velocity: RectVelocity::default(),
        }
    }

    pub fn set_target(&mut self, target: Rect) {
        self.target = normalized_rect(target);
    }

    pub fn snap(&mut self) {
        self.current = self.target;
        self.velocity = RectVelocity::default();
    }

    pub fn advance(&mut self, delta: Duration, config: SpringConfig) -> bool {
        if self.is_settled(config) {
            self.snap();
            return false;
        }

        let config = config.normalized();
        let seconds = delta.min(MAX_FRAME_DELTA).as_secs_f64();
        let steps = (seconds / MAX_STEP_SECONDS).ceil().max(1.0) as usize;
        let step = seconds / steps as f64;

        for _ in 0..steps {
            advance_value(
                &mut self.current.x,
                self.target.x,
                &mut self.velocity.x,
                step,
                config,
            );
            advance_value(
                &mut self.current.y,
                self.target.y,
                &mut self.velocity.y,
                step,
                config,
            );
            advance_value(
                &mut self.current.width,
                self.target.width,
                &mut self.velocity.width,
                step,
                config,
            );
            advance_value(
                &mut self.current.height,
                self.target.height,
                &mut self.velocity.height,
                step,
                config,
            );
        }

        if self.is_settled(config) {
            self.snap();
            false
        } else {
            true
        }
    }

    pub fn is_settled(&self, config: SpringConfig) -> bool {
        let config = config.normalized();
        rect_distance(self.current, self.target) <= config.position_tolerance
            && velocity_magnitude(self.velocity) <= config.velocity_tolerance
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClientSize {
    pub width: i32,
    pub height: i32,
}

impl ClientSize {
    pub fn from_rect(rect: Rect) -> Self {
        Self {
            width: (rect.width.round() as i32).max(1),
            height: (rect.height.round() as i32).max(1),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClientGeometry {
    pub last_configured_size: Option<ClientSize>,
    pub committed_size: Option<ClientSize>,
    waiting_since: Option<Duration>,
}

impl ClientGeometry {
    pub fn request_size(&mut self, size: ClientSize, now: Duration) -> bool {
        if self.last_configured_size == Some(size) {
            return false;
        }

        self.last_configured_size = Some(size);
        self.waiting_since = Some(now);
        true
    }

    pub fn commit(&mut self, size: ClientSize) -> bool {
        self.committed_size = Some(size);
        let matches_target = self.last_configured_size == Some(size);

        if matches_target {
            self.waiting_since = None;
        }

        matches_target
    }

    pub fn timed_out(&self, now: Duration) -> bool {
        self.waiting_since
            .is_some_and(|started| now.saturating_sub(started) >= CLIENT_COMMIT_TIMEOUT)
    }

    pub fn expire_wait(&mut self, now: Duration) -> Option<ClientSize> {
        if !self.timed_out(now) {
            return None;
        }

        self.waiting_since = None;
        self.last_configured_size
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationMode {
    Normal,
    Maximized,
    Fullscreen,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowGeometry {
    pub logical: Rect,
    pub visual: AnimatedRect,
    pub client: ClientGeometry,
    pub decorations: f64,
    pub presentation_changed: bool,
    mode: PresentationMode,
    zoom: Option<ZoomTransition>,
}

/// Maximized/fullscreen geometry and decorations share a clock, including on reversal.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ZoomTransition {
    from: Rect,
    decorations_from: f64,
    progress: AnimatedValue,
}

impl WindowGeometry {
    pub fn new(rect: Rect, committed_size: Option<ClientSize>) -> Self {
        let rect = normalized_rect(rect);

        Self {
            logical: rect,
            visual: AnimatedRect::new(rect),
            client: ClientGeometry {
                last_configured_size: committed_size,
                committed_size,
                waiting_since: None,
            },
            decorations: 1.0,
            presentation_changed: true,
            mode: PresentationMode::Normal,
            zoom: None,
        }
    }

    pub fn set_logical_target(&mut self, rect: Rect, now: Duration) -> Option<ClientSize> {
        let rect = normalized_rect(rect);
        let size = ClientSize::from_rect(rect);
        self.logical = rect;
        self.visual.set_target(rect);

        self.client.request_size(size, now).then_some(size)
    }

    pub fn set_presentation_target(
        &mut self,
        rect: Rect,
        fullscreen: bool,
        now: Duration,
    ) -> Option<ClientSize> {
        self.set_presentation_mode(
            rect,
            if fullscreen {
                PresentationMode::Fullscreen
            } else {
                PresentationMode::Normal
            },
            now,
        )
    }

    pub fn set_presentation_mode(
        &mut self,
        rect: Rect,
        mode: PresentationMode,
        now: Duration,
    ) -> Option<ClientSize> {
        let rect = normalized_rect(rect);
        if self.mode != mode || (self.zoom.is_some() && self.logical != rect) {
            let mut progress = AnimatedValue::new(0.0);
            progress.set_target(1.0);
            let from = self.visual.current;
            let distance = [
                rect.x - from.x,
                rect.y - from.y,
                rect.width - from.width,
                rect.height - from.height,
            ];
            let velocity = self.visual.velocity;
            let speed = [velocity.x, velocity.y, velocity.width, velocity.height];
            let squared_length = distance.iter().map(|value| value * value).sum::<f64>();
            if squared_length > 0.001 {
                let projected =
                    distance.iter().zip(speed).map(|(d, v)| d * v).sum::<f64>() / squared_length;
                // Keep forward momentum; soften a reversal without jumping position.
                progress.velocity = if projected < 0.0 {
                    projected * 0.35
                } else {
                    projected
                };
            }
            self.zoom = Some(ZoomTransition {
                from: self.visual.current,
                decorations_from: self.decorations,
                progress,
            });
        }
        self.mode = mode;
        self.logical = rect;
        self.visual.set_target(rect);
        self.presentation_size_request(now)
    }

    /// Request the destination once, independently of the presentation clock.
    /// The compositor coordinates commits; delaying this until settlement
    /// forces presentation to use an obsolete buffer for the entire resize.
    pub fn presentation_size_request(&mut self, now: Duration) -> Option<ClientSize> {
        let size = ClientSize::from_rect(self.logical);
        self.client.request_size(size, now).then_some(size)
    }

    pub fn is_zooming(&self) -> bool {
        self.zoom.is_some()
    }

    pub fn is_fullscreen(&self) -> bool {
        self.mode == PresentationMode::Fullscreen
    }

    pub fn settle_presentation(&mut self) {
        self.visual.snap();
        self.zoom = None;
        self.decorations = if self.is_fullscreen() { 0.0 } else { 1.0 };
        self.presentation_changed = true;
    }

    pub fn follow_pointer(&mut self, rect: Rect, now: Duration) -> Option<ClientSize> {
        self.zoom = None;
        let requested_size = self.set_logical_target(rect, now);
        self.visual.snap();
        self.decorations = if self.is_fullscreen() { 0.0 } else { 1.0 };
        self.presentation_changed = true;
        requested_size
    }

    pub fn advance(
        &mut self,
        delta: Duration,
        config: SpringConfig,
        animations_enabled: bool,
    ) -> bool {
        let previous = (self.visual.current, self.decorations);
        let active = self.advance_presentation(delta, config, animations_enabled);
        self.presentation_changed = previous != (self.visual.current, self.decorations);
        active
    }

    fn advance_presentation(
        &mut self,
        delta: Duration,
        config: SpringConfig,
        animations_enabled: bool,
    ) -> bool {
        if !animations_enabled {
            self.visual.snap();
            self.decorations = if self.is_fullscreen() { 0.0 } else { 1.0 };
            self.zoom = None;
            return false;
        }

        if let Some(zoom) = &mut self.zoom {
            let target = self.visual.target;
            // Spring tolerances describe pixels and pixels/second, whereas this
            // spring advances unit progress. Without conversion it snaps several
            // pixels early on large zooms, visibly shifting the trailing edge.
            let travel = rect_distance(zoom.from, target)
                .max((zoom.from.x + zoom.from.width - target.x - target.width).abs())
                .max((zoom.from.y + zoom.from.height - target.y - target.height).abs())
                .max(1.0);
            let config = config.normalized();
            let progress_config = SpringConfig {
                position_tolerance: config.position_tolerance / travel,
                velocity_tolerance: config.velocity_tolerance / travel,
                ..config
            };
            let active = zoom.progress.advance(delta, progress_config);
            let p = zoom.progress.current;
            let lerp = |a: f64, b: f64| a + (b - a) * p;
            self.visual.current = Rect::new(
                lerp(zoom.from.x, target.x),
                lerp(zoom.from.y, target.y),
                lerp(zoom.from.width, target.width),
                lerp(zoom.from.height, target.height),
            );
            self.visual.velocity = RectVelocity {
                x: (target.x - zoom.from.x) * zoom.progress.velocity,
                y: (target.y - zoom.from.y) * zoom.progress.velocity,
                width: (target.width - zoom.from.width) * zoom.progress.velocity,
                height: (target.height - zoom.from.height) * zoom.progress.velocity,
            };
            self.decorations = lerp(
                zoom.decorations_from,
                if self.mode == PresentationMode::Fullscreen {
                    0.0
                } else {
                    1.0
                },
            );
            if !active {
                self.zoom = None;
                self.visual.snap();
            }
            return active;
        }

        self.visual.advance(delta, config)
    }

    pub fn inverse_visual_point(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let visual = self.visual.current;
        let (scale_x, scale_y) = self.visual_scale()?;

        Some(((x - visual.x) / scale_x, (y - visual.y) / scale_y))
    }

    pub fn visual_scale(&self) -> Option<(f64, f64)> {
        let source = self.client.committed_size?;
        let visual = self.visual.current;
        if source.width <= 0 || source.height <= 0 || visual.width <= 0.0 || visual.height <= 0.0 {
            return None;
        }

        Some((
            visual.width / f64::from(source.width),
            visual.height / f64::from(source.height),
        ))
    }
}

fn advance_value(
    current: &mut f64,
    target: f64,
    velocity: &mut f64,
    delta: f64,
    config: SpringConfig,
) {
    let acceleration =
        (-config.stiffness * (*current - target) - config.damping * *velocity) / config.mass;
    *velocity += acceleration * delta;
    let previous_error = *current - target;
    *current += *velocity * delta;
    let current_error = *current - target;

    if previous_error != 0.0 && previous_error.signum() != current_error.signum() {
        *current = target;
        *velocity = 0.0;
    }
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

fn normalized_rect(rect: Rect) -> Rect {
    Rect::new(
        finite_or(rect.x, 0.0),
        finite_or(rect.y, 0.0),
        positive_or(rect.width, 1.0),
        positive_or(rect.height, 1.0),
    )
}

fn rect_distance(left: Rect, right: Rect) -> f64 {
    (left.x - right.x)
        .abs()
        .max((left.y - right.y).abs())
        .max((left.width - right.width).abs())
        .max((left.height - right.height).abs())
}

fn velocity_magnitude(velocity: RectVelocity) -> f64 {
    velocity
        .x
        .abs()
        .max(velocity.y.abs())
        .max(velocity.width.abs())
        .max(velocity.height.abs())
}

fn positive_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

fn non_negative_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        fallback
    }
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returning_fullscreen_window_does_not_resume_hidden_zoom() {
        let normal = Rect::new(100.0, 50.0, 900.0, 700.0);
        let fullscreen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let mut geometry = WindowGeometry::new(normal, None);
        geometry.set_presentation_mode(fullscreen, PresentationMode::Fullscreen, Duration::ZERO);
        geometry.advance(Duration::from_millis(16), SpringConfig::default(), true);
        assert!(geometry.is_zooming());

        geometry.settle_presentation();

        assert_eq!(geometry.visual.current, fullscreen);
        assert_eq!(geometry.decorations, 0.0);
        assert!(!geometry.is_zooming());
        assert!(!geometry.advance(Duration::from_millis(16), SpringConfig::default(), true));
    }

    #[test]
    fn hiding_maximized_window_finishes_resize_without_hiding_decorations() {
        let normal = Rect::new(100.0, 50.0, 900.0, 700.0);
        let maximized = Rect::new(0.0, 40.0, 1920.0, 1040.0);
        let mut geometry = WindowGeometry::new(normal, None);
        geometry.set_presentation_mode(maximized, PresentationMode::Maximized, Duration::ZERO);
        geometry.advance(Duration::from_millis(16), SpringConfig::default(), true);
        assert!(geometry.is_zooming());

        geometry.settle_presentation();

        assert_eq!(geometry.visual.current, maximized);
        assert_eq!(geometry.decorations, 1.0);
        assert!(!geometry.is_zooming());
    }

    #[test]
    fn maximized_zoom_keeps_decorations_and_restores_through_fullscreen() {
        let normal = Rect::new(800.0, 50.0, 600.0, 700.0);
        let maximized = Rect::new(10.0, 50.0, 1580.0, 740.0);
        let fullscreen = Rect::new(0.0, 0.0, 1600.0, 800.0);
        let mut geometry = WindowGeometry::new(normal, None);
        for (mode, rect) in [
            (PresentationMode::Maximized, maximized),
            (PresentationMode::Fullscreen, fullscreen),
            (PresentationMode::Maximized, maximized),
            (PresentationMode::Normal, normal),
        ] {
            let before = geometry.visual.current;
            let decorations = geometry.decorations;
            geometry.set_presentation_mode(rect, mode, Duration::ZERO);
            assert_eq!(geometry.visual.current, before);
            assert_eq!(geometry.decorations, decorations);
            for _ in 0..1000 {
                if !geometry.advance(Duration::from_millis(8), SpringConfig::default(), true) {
                    break;
                }
                if mode != PresentationMode::Fullscreen && decorations == 1.0 {
                    assert_eq!(geometry.decorations, 1.0);
                }
            }
            assert_eq!(geometry.visual.current, rect);
            assert_eq!(
                geometry.decorations,
                if mode == PresentationMode::Fullscreen {
                    0.0
                } else {
                    1.0
                }
            );
            assert!(!geometry.is_zooming());
            assert_eq!(
                geometry.is_fullscreen(),
                mode == PresentationMode::Fullscreen
            );
        }
    }

    #[test]
    fn reversing_maximization_preserves_visual_position_and_decoration() {
        let normal = Rect::new(800.0, 50.0, 600.0, 700.0);
        let mut geometry = WindowGeometry::new(normal, None);
        geometry.set_presentation_mode(
            Rect::new(10.0, 50.0, 1580.0, 740.0),
            PresentationMode::Maximized,
            Duration::ZERO,
        );
        geometry.advance(Duration::from_millis(80), SpringConfig::default(), true);
        let before = geometry.visual.current;
        geometry.set_presentation_mode(normal, PresentationMode::Normal, Duration::from_millis(80));
        assert_eq!(geometry.visual.current, before);
        assert_eq!(geometry.decorations, 1.0);
        geometry.advance(Duration::ZERO, SpringConfig::default(), false);
        assert_eq!(geometry.visual.current, normal);
        assert_eq!(geometry.decorations, 1.0);
    }

    #[test]
    fn zoom_finishes_without_a_visible_final_edge_jump() {
        let tiled = Rect::new(12.0, 12.0, 942.0, 1056.0);
        let fullscreen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        for (start, target, fullscreen_target) in
            [(tiled, fullscreen, true), (fullscreen, tiled, false)]
        {
            let mut geometry = WindowGeometry::new(start, None);
            if !fullscreen_target {
                geometry.set_presentation_target(start, true, Duration::ZERO);
                geometry.advance(Duration::ZERO, SpringConfig::default(), false);
            }
            geometry.set_presentation_target(target, fullscreen_target, Duration::ZERO);
            let mut finished = false;
            for _ in 0..1000 {
                let before = geometry.visual.current;
                if !geometry.advance(
                    Duration::from_secs_f64(1.0 / 144.0),
                    SpringConfig::default(),
                    true,
                ) {
                    let right_before = before.x + before.width;
                    let right_after = target.x + target.width;
                    assert!(
                        (right_after - right_before).abs() < 0.5,
                        "final right-edge jump: {} pixels",
                        (right_after - right_before).abs()
                    );
                    finished = true;
                    break;
                }
            }
            assert!(finished);
        }
    }

    #[test]
    fn zoom_geometry_and_decorations_share_progress() {
        let start = Rect::new(200.0, 40.0, 600.0, 800.0);
        let end = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let mut geometry = WindowGeometry::new(start, Some(ClientSize::from_rect(start)));
        geometry.set_presentation_target(end, true, Duration::ZERO);
        for _ in 0..20 {
            geometry.advance(Duration::from_millis(8), SpringConfig::default(), true);
            let progress = 1.0 - geometry.decorations;
            let visual = geometry.visual.current;
            assert!((visual.x - (start.x + (end.x - start.x) * progress)).abs() < 1e-9);
            assert!(
                (visual.width - (start.width + (end.width - start.width) * progress)).abs() < 1e-9
            );
        }
    }

    #[test]
    fn rapid_zoom_reversal_preserves_presented_frame_and_reaches_latest_target() {
        let tiled = Rect::new(600.0, 20.0, 600.0, 900.0);
        let fullscreen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let mut geometry = WindowGeometry::new(tiled, None);
        for enabled in [true, false, true, false] {
            let before = (geometry.visual.current, geometry.decorations);
            geometry.set_presentation_target(
                if enabled { fullscreen } else { tiled },
                enabled,
                Duration::ZERO,
            );
            assert_eq!((geometry.visual.current, geometry.decorations), before);
            geometry.advance(Duration::from_millis(48), SpringConfig::default(), true);
        }
        for _ in 0..240 {
            geometry.advance(Duration::from_millis(8), SpringConfig::default(), true);
        }
        assert_eq!(geometry.visual.current, tiled);
        assert_eq!(geometry.decorations, 1.0);
        assert!(!geometry.is_zooming());
    }

    #[test]
    fn zoom_configures_destination_once_before_animating() {
        let start = Rect::new(0.0, 0.0, 600.0, 800.0);
        let end = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let mut geometry = WindowGeometry::new(start, Some(ClientSize::from_rect(start)));
        assert_eq!(
            geometry.set_presentation_target(end, true, Duration::ZERO),
            Some(ClientSize::from_rect(end))
        );
        for frame in 0..240 {
            let now = Duration::from_millis(frame * 8);
            geometry.advance(Duration::from_millis(8), SpringConfig::default(), true);
            assert_eq!(geometry.presentation_size_request(now), None);
        }
        assert_eq!(geometry.visual.current, end);
        assert_eq!(
            geometry.set_presentation_target(start, false, Duration::from_secs(2)),
            Some(ClientSize::from_rect(start))
        );
    }

    #[test]
    fn animated_resize_configures_each_new_target_without_framewise_requests() {
        let start = Rect::new(10.0, 10.0, 600.0, 800.0);
        let middle = Rect::new(10.0, 10.0, 900.0, 800.0);
        let end = Rect::new(10.0, 10.0, 450.0, 800.0);
        let mut geometry = WindowGeometry::new(start, Some(ClientSize::from_rect(start)));
        assert_eq!(
            geometry.set_presentation_mode(middle, PresentationMode::Normal, Duration::ZERO),
            Some(ClientSize::from_rect(middle))
        );
        geometry.advance(Duration::from_millis(64), SpringConfig::default(), true);
        assert_eq!(
            geometry.presentation_size_request(Duration::from_millis(64)),
            None
        );
        assert_eq!(
            geometry.set_presentation_mode(
                end,
                PresentationMode::Normal,
                Duration::from_millis(64)
            ),
            Some(ClientSize::from_rect(end))
        );
        let mut requests = Vec::new();
        for frame in 1..240 {
            geometry.advance(Duration::from_millis(8), SpringConfig::default(), true);
            if let Some(size) =
                geometry.presentation_size_request(Duration::from_millis(64 + frame * 8))
            {
                assert_eq!(geometry.visual.current.width, end.width);
                requests.push(size);
            }
        }
        assert!(requests.is_empty());
    }

    #[test]
    fn pointer_resize_stays_live_and_reduced_motion_configures_immediately() {
        let start = Rect::new(0.0, 0.0, 600.0, 800.0);
        let end = Rect::new(0.0, 0.0, 900.0, 800.0);
        let mut pointer = WindowGeometry::new(start, Some(ClientSize::from_rect(start)));
        assert_eq!(
            pointer.follow_pointer(end, Duration::ZERO),
            Some(ClientSize::from_rect(end))
        );
        let mut reduced = WindowGeometry::new(start, Some(ClientSize::from_rect(start)));
        assert_eq!(
            reduced.set_presentation_target(end, true, Duration::ZERO),
            Some(ClientSize::from_rect(end))
        );
        reduced.advance(Duration::ZERO, SpringConfig::default(), false);
        assert_eq!(reduced.presentation_size_request(Duration::ZERO), None);
    }

    #[test]
    fn left_and_right_zoom_edges_do_not_reverse_direction() {
        let fullscreen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        for tiled in [
            Rect::new(12.0, 12.0, 942.0, 1056.0),
            Rect::new(966.0, 12.0, 942.0, 1056.0),
        ] {
            for rate in [60, 144] {
                let mut geometry = WindowGeometry::new(tiled, Some(ClientSize::from_rect(tiled)));
                for (target, enabled) in [(fullscreen, true), (tiled, false)] {
                    geometry.set_presentation_target(target, enabled, Duration::ZERO);
                    let mut previous = geometry.visual.current;
                    for _ in 0..rate * 2 {
                        geometry.advance(
                            Duration::from_secs_f64(1.0 / f64::from(rate)),
                            SpringConfig::default(),
                            true,
                        );
                        let current = geometry.visual.current;
                        let edges = |rect: Rect| {
                            [rect.x, rect.y, rect.x + rect.width, rect.y + rect.height]
                        };
                        for ((before, after), end) in edges(previous)
                            .into_iter()
                            .zip(edges(current))
                            .zip(edges(target))
                        {
                            assert!((after - end).abs() <= (before - end).abs() + 1e-9);
                        }
                        previous = current;
                    }
                    assert_eq!(geometry.visual.current, target);
                }
            }
        }
    }

    #[test]
    fn drag_takes_ownership_from_an_unfinished_zoom() {
        let start = Rect::new(0.0, 0.0, 600.0, 800.0);
        let mut geometry = WindowGeometry::new(start, None);
        geometry.set_presentation_target(Rect::new(0.0, 0.0, 1920.0, 1080.0), true, Duration::ZERO);
        geometry.advance(Duration::from_millis(32), SpringConfig::default(), true);
        geometry.set_presentation_target(start, false, Duration::ZERO);
        let dragged = Rect::new(40.0, 50.0, 500.0, 700.0);
        geometry.follow_pointer(dragged, Duration::ZERO);
        assert!(!geometry.advance(Duration::from_millis(16), SpringConfig::default(), true));
        assert_eq!(geometry.visual.current, dragged);
    }

    #[test]
    fn reduced_motion_finishes_zoom_and_decorations_together() {
        let mut geometry = WindowGeometry::new(Rect::new(100.0, 20.0, 600.0, 800.0), None);
        let target = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        geometry.set_presentation_target(target, true, Duration::ZERO);
        geometry.advance(Duration::ZERO, SpringConfig::default(), false);
        assert_eq!(geometry.visual.current, target);
        assert_eq!(geometry.decorations, 0.0);
        assert!(!geometry.is_zooming());
    }

    #[test]
    fn relayout_during_zoom_does_not_restart_its_clock() {
        let start = Rect::new(600.0, 20.0, 600.0, 900.0);
        let target = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let mut uninterrupted = WindowGeometry::new(start, None);
        uninterrupted.set_presentation_target(target, true, Duration::ZERO);
        let mut relayout = uninterrupted;
        for frame in 0..120 {
            let now = Duration::from_millis(frame * 8);
            relayout.set_presentation_target(target, true, now);
            relayout.advance(Duration::from_millis(8), SpringConfig::default(), true);
            uninterrupted.advance(Duration::from_millis(8), SpringConfig::default(), true);
            assert_eq!(relayout.visual, uninterrupted.visual);
            assert_eq!(relayout.decorations, uninterrupted.decorations);
        }
    }

    #[test]
    fn zoom_at_different_refresh_rates_has_comparable_progress() {
        let start = Rect::new(600.0, 20.0, 600.0, 900.0);
        let target = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let mut progress = Vec::new();
        for rate in [60, 120, 144] {
            let mut geometry = WindowGeometry::new(start, None);
            geometry.set_presentation_target(target, true, Duration::ZERO);
            for _ in 0..rate / 6 {
                geometry.advance(
                    Duration::from_secs_f64(1.0 / f64::from(rate)),
                    SpringConfig::default(),
                    true,
                );
            }
            progress.push(geometry.decorations);
        }
        assert!((progress[0] - progress[1]).abs() < 0.01);
        assert!((progress[0] - progress[2]).abs() < 0.01);
    }

    #[test]
    fn final_frame_invalidates_clip_even_when_zoom_has_settled() {
        let start = Rect::new(0.0, 0.0, 600.0, 800.0);
        let mut geometry = WindowGeometry::new(start, None);
        geometry.set_presentation_target(start, true, Duration::ZERO);
        for _ in 0..1000 {
            if !geometry.advance(Duration::from_millis(8), SpringConfig::default(), true) {
                assert!(geometry.presentation_changed);
                assert_eq!(geometry.decorations, 0.0);
                geometry.advance(Duration::from_millis(8), SpringConfig::default(), true);
                assert!(!geometry.presentation_changed);
                return;
            }
        }
        panic!("zoom failed to settle");
    }

    #[test]
    fn scalar_spring_retargets_from_current_motion() {
        let mut value = AnimatedValue::new(0.0);
        value.set_target(1_000.0);
        value.advance(Duration::from_millis(40), SpringConfig::default());
        let current = value.current;
        let velocity = value.velocity;

        value.retarget_preserving_motion(2_000.0);

        assert_eq!(value.current, current);
        assert_eq!(value.velocity, velocity);
        assert_eq!(value.target, 2_000.0);
    }

    #[test]
    fn scalar_spring_softens_opposing_velocity_on_reversal() {
        let mut value = AnimatedValue::new(0.0);
        value.set_target(1_000.0);
        value.advance(Duration::from_millis(40), SpringConfig::default());
        let current = value.current;
        let velocity = value.velocity;

        value.retarget_preserving_motion(-500.0);

        assert!(value.velocity > 0.0);
        assert_eq!(value.velocity, velocity * 0.35);
        assert_eq!(value.current, current);
    }

    #[test]
    fn scalar_spring_does_not_overshoot_target() {
        let mut value = AnimatedValue::new(0.0);
        value.velocity = 20_000.0;
        value.set_target(100.0);

        for _ in 0..60 {
            value.advance(Duration::from_secs_f64(1.0 / 60.0), SpringConfig::default());
            assert!(value.current <= 100.0);
        }
    }

    #[test]
    fn logical_target_changes_without_jumping_visual_geometry() {
        let initial = Rect::new(0.0, 0.0, 400.0, 300.0);
        let target = Rect::new(200.0, 100.0, 800.0, 600.0);
        let mut geometry = WindowGeometry::new(initial, Some(ClientSize::from_rect(initial)));

        assert_eq!(
            geometry.set_logical_target(target, Duration::ZERO),
            Some(ClientSize::from_rect(target))
        );
        assert_eq!(geometry.logical, target);
        assert_eq!(geometry.visual.current, initial);
        assert_eq!(geometry.visual.target, target);
    }

    #[test]
    fn repeated_target_does_not_emit_another_configure() {
        let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
        let mut geometry = WindowGeometry::new(rect, None);

        assert!(geometry.set_logical_target(rect, Duration::ZERO).is_some());
        assert!(
            geometry
                .set_logical_target(rect, Duration::from_millis(16))
                .is_none()
        );
    }

    #[test]
    fn matching_commit_clears_timeout_without_restarting_visual_motion() {
        let initial = Rect::new(0.0, 0.0, 400.0, 300.0);
        let target = Rect::new(100.0, 50.0, 800.0, 600.0);
        let mut geometry = WindowGeometry::new(initial, None);
        let requested = geometry.set_logical_target(target, Duration::ZERO).unwrap();
        geometry.advance(Duration::from_millis(100), SpringConfig::default(), true);
        let visual_before_commit = geometry.visual;

        assert!(geometry.client.commit(requested));
        assert_eq!(geometry.visual, visual_before_commit);
        assert!(!geometry.client.timed_out(Duration::from_secs(1)));
    }

    #[test]
    fn slow_client_times_out_after_half_a_second() {
        let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
        let mut client = ClientGeometry::default();
        client.request_size(ClientSize::from_rect(rect), Duration::ZERO);

        assert!(!client.timed_out(Duration::from_millis(499)));
        assert!(client.timed_out(Duration::from_millis(500)));
        assert_eq!(
            client.expire_wait(Duration::from_millis(500)),
            Some(ClientSize::from_rect(rect))
        );
        assert!(!client.timed_out(Duration::from_secs(1)));
    }

    #[test]
    fn inverse_visual_point_matches_stretched_transform() {
        let initial = Rect::new(100.0, 50.0, 400.0, 300.0);
        let mut geometry = WindowGeometry::new(initial, Some(ClientSize::from_rect(initial)));
        geometry.visual.current = Rect::new(200.0, 100.0, 600.0, 600.0);

        assert_eq!(
            geometry.inverse_visual_point(600.0, 500.0),
            Some((400.0 * 2.0 / 3.0, 200.0))
        );
    }

    #[test]
    fn default_spring_is_stable_across_refresh_rates_and_does_not_overshoot() {
        let start = Rect::new(0.0, 0.0, 100.0, 100.0);
        let target = Rect::new(500.0, 300.0, 900.0, 700.0);
        let mut sixty = AnimatedRect::new(start);
        let mut one_forty_four = AnimatedRect::new(start);
        sixty.set_target(target);
        one_forty_four.set_target(target);

        for _ in 0..120 {
            sixty.advance(Duration::from_secs_f64(1.0 / 60.0), SpringConfig::default());
            assert!(sixty.current.x <= target.x);
            assert!(sixty.current.width <= target.width);
        }
        for _ in 0..288 {
            one_forty_four.advance(
                Duration::from_secs_f64(1.0 / 144.0),
                SpringConfig::default(),
            );
        }

        assert!(sixty.is_settled(SpringConfig::default()));
        assert!(one_forty_four.is_settled(SpringConfig::default()));
        assert_eq!(sixty.current, target);
        assert_eq!(one_forty_four.current, target);
    }

    #[test]
    fn default_spring_completes_most_motion_within_two_hundred_milliseconds() {
        let start = Rect::new(0.0, 0.0, 100.0, 100.0);
        let target = Rect::new(500.0, 300.0, 900.0, 700.0);
        let mut animated = AnimatedRect::new(start);
        animated.set_target(target);

        for _ in 0..12 {
            animated.advance(Duration::from_secs_f64(1.0 / 60.0), SpringConfig::default());
        }

        assert!(animated.current.x >= 475.0);
        assert!(animated.current.y >= 285.0);
        assert!(animated.current.width >= 860.0);
        assert!(animated.current.height >= 670.0);
        assert!(animated.current.x <= target.x);
        assert!(animated.current.y <= target.y);
        assert!(animated.current.width <= target.width);
        assert!(animated.current.height <= target.height);
    }

    #[test]
    fn disabled_animation_snaps_to_target() {
        let mut geometry = WindowGeometry::new(Rect::new(0.0, 0.0, 100.0, 100.0), None);
        let target = Rect::new(100.0, 100.0, 500.0, 400.0);
        geometry.set_logical_target(target, Duration::ZERO);

        assert!(!geometry.advance(Duration::from_millis(16), SpringConfig::default(), false));
        assert_eq!(geometry.visual.current, target);
    }
}
