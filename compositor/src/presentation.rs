//! Presentation geometry is rounded once, in output pixels, never first in
//! logical pixels. Content, clips and decorations share those exact edges.
use std::time::{Duration, Instant};

use smithay::{
    backend::renderer::{
        element::{Element, Id, Kind, RenderElement},
        gles::{
            GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform,
            element::PixelShaderElement,
        },
        utils::{CommitCounter, DamageSet},
    },
    utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Transform},
};

pub(crate) const HANDOFF: Duration = Duration::from_millis(80);
pub(crate) const SNAPSHOT_BUDGET: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct CallbackClock {
    last_sent: Option<Instant>,
}

impl CallbackClock {
    pub(crate) fn deadline(&self, now: Instant, interval: Duration) -> Instant {
        self.last_sent
            .map_or(now, |last| (last + interval).max(now))
    }

    pub(crate) fn sent(&mut self, now: Instant) {
        self.last_sent = Some(now);
    }
}

pub(crate) fn frame_delta(last: &mut Instant, now: Instant) -> Duration {
    let delta = now.saturating_duration_since(*last);
    *last = (*last).max(now);
    delta
}

pub(crate) fn physical_rect(
    rect: ferese_layout::Rect,
    origin: Point<i32, Logical>,
    scale: f64,
) -> Rectangle<i32, Physical> {
    let left = ((rect.x - f64::from(origin.x)) * scale).round() as i32;
    let top = ((rect.y - f64::from(origin.y)) * scale).round() as i32;
    let right = ((rect.x + rect.width - f64::from(origin.x)) * scale).round() as i32;
    let bottom = ((rect.y + rect.height - f64::from(origin.y)) * scale).round() as i32;
    Rectangle::new(
        (left, top).into(),
        ((right - left).max(1), (bottom - top).max(1)).into(),
    )
}

pub(crate) fn handoff_alpha(elapsed: Duration) -> f32 {
    let progress = (elapsed.as_secs_f64() / HANDOFF.as_secs_f64()).clamp(0.0, 1.0);
    (1.0 - progress * progress * (3.0 - 2.0 * progress)) as f32
}

pub(crate) fn resize_needs_old_frame(
    visual: ferese_layout::Rect,
    target: ferese_layout::Rect,
    buffer_width: i32,
    buffer_height: i32,
) -> bool {
    // Some clients round their configured dimensions to a character grid.
    // Do not retain a snapshot forever for that permanent size difference.
    visual.width > target.width.max(f64::from(buffer_width)) + 0.5
        || visual.height > target.height.max(f64::from(buffer_height)) + 0.5
}

pub(crate) fn snapshot_covers_source(
    snapshot: smithay::utils::Size<i32, Buffer>,
    source: smithay::utils::Size<i32, Physical>,
    snapshot_scale: f64,
    scale: f64,
) -> bool {
    (snapshot_scale - scale).abs() < 0.001 && snapshot.w >= source.w && snapshot.h >= source.h
}

pub(crate) fn advance_handoff(
    elapsed: &mut Duration,
    last: &mut Option<Duration>,
    now: Duration,
    blocked: bool,
    speed: f64,
) -> bool {
    if blocked {
        // A retarget must not consume its client's waiting time as fade time.
        *last = None;
    } else {
        if let Some(previous) = last.replace(now) {
            *elapsed += now.saturating_sub(previous).mul_f64(speed);
        }
    }
    *elapsed < HANDOFF
}

/// A shader canvas whose damage/paint destination uses the shared pixel edges.
#[derive(Clone, Debug)]
pub(crate) struct PhysicalShaderElement {
    pub inner: PixelShaderElement,
    pub geometry: Rectangle<i32, Physical>,
}

impl Element for PhysicalShaderElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn geometry(&self, _: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry
    }

    fn damage_since(
        &self,
        _: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        if commit == Some(self.current_commit()) {
            DamageSet::default()
        } else {
            DamageSet::from_slice(&[Rectangle::from_size(self.geometry.size)])
        }
    }
}

impl RenderElement<GlesRenderer> for PhysicalShaderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        self.inner.draw(frame, src, dst, damage, opaque)
    }
}

/// Stable native-pixel texture element. Unlike a resized client buffer, an old
/// frame is only translated/cropped; it never changes the size of its glyphs.
#[derive(Clone, Debug)]
pub(crate) struct NativeTextureElement {
    pub id: Id,
    pub commit: CommitCounter,
    pub texture: GlesTexture,
    pub geometry: Rectangle<i32, Physical>,
    pub source: Rectangle<f64, Buffer>,
    pub alpha: f32,
    pub program: Option<GlesTexProgram>,
    pub uniforms: Vec<Uniform<'static>>,
}

impl Element for NativeTextureElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.source
    }

    fn geometry(&self, _: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometry
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }

    fn damage_since(
        &self,
        _: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        if commit == Some(self.commit) {
            DamageSet::default()
        } else {
            DamageSet::from_slice(&[Rectangle::from_size(self.geometry.size)])
        }
    }
}

impl RenderElement<GlesRenderer> for NativeTextureElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque,
            Transform::Normal,
            self.alpha,
            self.program.as_ref(),
            &self.uniforms,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversing_a_grow_replaces_the_smaller_snapshot() {
        assert!(!snapshot_covers_source(
            (500, 600).into(),
            (1000, 600).into(),
            1.0,
            1.0
        ));
        assert!(snapshot_covers_source(
            (1000, 600).into(),
            (500, 600).into(),
            1.0,
            1.0
        ));
        assert!(!snapshot_covers_source(
            (1000, 600).into(),
            (500, 600).into(),
            1.0,
            1.8
        ));
    }

    #[test]
    fn shrinking_keeps_old_pixels_until_destination_covers_bounds() {
        let target = ferese_layout::Rect::new(0.0, 0.0, 500.0, 600.0);
        let visual = ferese_layout::Rect::new(0.0, 0.0, 750.0, 600.0);
        assert!(resize_needs_old_frame(visual, target, 500, 600));
        assert!(!resize_needs_old_frame(target, target, 500, 600));
        // Character-grid clients must still release the bounded snapshot.
        assert!(!resize_needs_old_frame(target, target, 492, 592));
        let growing_target = ferese_layout::Rect::new(0.0, 0.0, 1000.0, 600.0);
        assert!(!resize_needs_old_frame(visual, growing_target, 1000, 600));
    }

    #[test]
    fn fresh_snapshot_and_retarget_wait_do_not_skip_the_handoff() {
        let mut elapsed = Duration::ZERO;
        let mut last = None;
        assert!(advance_handoff(
            &mut elapsed,
            &mut last,
            Duration::from_secs(5),
            false,
            1.0
        ));
        assert_eq!(elapsed, Duration::ZERO);
        assert!(advance_handoff(
            &mut elapsed,
            &mut last,
            Duration::from_millis(5010),
            false,
            1.0
        ));
        assert_eq!(elapsed, Duration::from_millis(10));
        advance_handoff(&mut elapsed, &mut last, Duration::from_secs(6), true, 1.0);
        advance_handoff(&mut elapsed, &mut last, Duration::from_secs(7), false, 1.0);
        assert_eq!(elapsed, Duration::from_millis(10));
        assert!(!advance_handoff(
            &mut elapsed,
            &mut last,
            Duration::from_millis(7070),
            false,
            1.0
        ));
    }

    #[test]
    fn no_damage_requests_are_paced_without_buffer_submission() {
        let start = Instant::now();
        let refresh = Duration::from_millis(16);
        let mut clock = CallbackClock::default();
        assert_eq!(clock.deadline(start, refresh), start);
        clock.sent(start);
        for millisecond in 1..16 {
            assert_eq!(
                clock.deadline(start + Duration::from_millis(millisecond), refresh),
                start + refresh
            );
        }
        let late = start + Duration::from_millis(50);
        assert_eq!(clock.deadline(late, refresh), late);
        clock.sent(late);
        assert_eq!(clock.deadline(late, refresh), late + refresh);
    }

    #[test]
    fn independent_output_callback_clocks_do_not_sum_their_rates() {
        let start = Instant::now();
        let mut slow = CallbackClock::default();
        let mut fast = CallbackClock::default();
        slow.sent(start);
        fast.sent(start);
        fast.sent(start + Duration::from_millis(8));
        assert_eq!(
            slow.deadline(start + Duration::from_millis(8), Duration::from_millis(16)),
            start + Duration::from_millis(16)
        );
        assert_eq!(
            fast.deadline(start + Duration::from_millis(8), Duration::from_millis(8)),
            start + Duration::from_millis(16)
        );
    }

    #[test]
    fn mixed_refresh_outputs_do_not_double_the_animation_clock() {
        let start = Instant::now();
        let mut last = start;
        let mut elapsed = Duration::ZERO;
        for ms in [8, 16, 16, 24, 32, 32] {
            elapsed += frame_delta(&mut last, start + Duration::from_millis(ms));
        }
        assert_eq!(elapsed, Duration::from_millis(32));
        assert_eq!(frame_delta(&mut last, start), Duration::ZERO);
        assert_eq!(last, start + Duration::from_millis(32));
    }

    #[test]
    fn fractional_motion_does_not_quantize_to_logical_pixels() {
        let rect = ferese_layout::Rect::new(0.3, 0.3, 100.2, 80.2);
        assert_eq!(physical_rect(rect, (0, 0).into(), 1.8).loc, (1, 1).into());
        assert_eq!(
            physical_rect(rect, (0, 0).into(), 1.8).size,
            (180, 144).into()
        );
    }

    #[test]
    fn adjacent_frames_share_edges_at_all_output_scales() {
        for scale in [1.0, 1.25, 1.5, 1.8, 2.0] {
            let left = physical_rect(
                ferese_layout::Rect::new(13.3, 0.0, 500.4, 800.0),
                (0, 0).into(),
                scale,
            );
            let right = physical_rect(
                ferese_layout::Rect::new(513.7, 0.0, 500.4, 800.0),
                (0, 0).into(),
                scale,
            );
            assert_eq!(left.loc.x + left.size.w, right.loc.x);
        }
    }

    #[test]
    fn handoff_has_bounded_lifetime_and_no_alpha_jump() {
        assert_eq!(handoff_alpha(Duration::ZERO), 1.0);
        assert_eq!(handoff_alpha(HANDOFF), 0.0);
        assert_eq!(handoff_alpha(Duration::from_secs(1)), 0.0);
        assert!(handoff_alpha(Duration::from_millis(1)) > 0.99);
    }
}
