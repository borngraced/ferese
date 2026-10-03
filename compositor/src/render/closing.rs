use super::*;
use ferese_animation::{AnimatedRect, AnimatedValue, CrossingPolicy, SpringConfig};
use ferese_core::OutputId;
use ferese_layout::WindowId;

/// Detached presentation only: no live window, input, focus, or layout entry.
#[derive(Clone)]
pub(crate) struct ClosedWindow {
    pub id: WindowId,
    pub output: OutputId,
    pub below: Option<WindowId>,
    pub bounds: AnimatedRect,
    pub opacity: AnimatedValue,
    pub snapshot: ResizeSnapshot,
    pub source: Rectangle<f64, Buffer>,
    pub radius: f64,
    pub shape: CornerShape,
    pub focus: f64,
    pub decorations: f64,
    pub dim: f64,
    pub fill: Option<[f32; 4]>,
    pub material: Option<(
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        crate::effects::SemanticRole,
        u64,
        f32,
    )>,
}

impl ClosedWindow {
    pub fn advance(&mut self, delta: Duration, spring: SpringConfig) -> bool {
        self.bounds.advance(delta, spring);
        let opacity = self.opacity.advance_with_policy(
            delta,
            SpringConfig {
                position_tolerance: 0.001,
                velocity_tolerance: 0.001,
                ..spring
            },
            CrossingPolicy::NoCrossing,
        );
        self.snapshot.commit.increment();
        // Once invisible there is no reason to keep a texture (including with
        // intentionally undamped, overshooting geometry configurations).
        opacity
    }
}

pub(super) fn elements(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
    below: Option<WindowId>,
    live: &std::collections::HashSet<WindowId>,
    delta: Duration,
) -> Vec<AnimatedWindowRenderElement> {
    if state.session_lock.active() {
        return Vec::new();
    }
    let Some(output_id) = state.output_id(output) else {
        return Vec::new();
    };
    let Some(output_geometry) = state.space.output_geometry(output) else {
        return Vec::new();
    };
    let scale = output.current_scale().fractional_scale();
    let windows: Vec<_> = state
        .render
        .closing
        .iter()
        .filter(|window| {
            window.output == output_id
                && window.below.filter(|id| live.contains(id)) == below
                && window.snapshot.context == renderer.context_id().erased()
        })
        .cloned()
        .collect();
    let mut elements = Vec::new();
    for mut window in windows {
        if !delta.is_zero() {
            window.advance(delta, state.spring_config);
        }
        let visual = window.bounds.current;
        let corners = RoundedRect::new(visual, output_geometry.loc, scale, window.radius).with_shape(window.shape);
        let constrain = rounded_visual_rect(visual, output_geometry.loc);
        let Some(programs) = corner_program(&mut state.render, renderer, window.shape) else {
            continue;
        };
        let alpha = window.opacity.current.clamp(0.0, 1.0) as f32;
        if let Some(dim) = window_tint_element(
            &mut state.render,
            renderer,
            window.id,
            constrain,
            corners,
            [0.0, 0.0, 0.0, window.dim as f32 * alpha],
            false,
            output,
        ) {
            elements.push(dim.into());
        }
        let theme = &state.theme_settings;
        if let Some(border) = window_border_element(
            &mut state.render,
            theme,
            renderer,
            window.id,
            constrain,
            corners,
            scale,
            theme.border_width + (theme.focus_ring_width - theme.border_width) * window.focus,
            theme.border_color.0,
            theme.border_gradient,
            window.focus as f32,
            alpha * window.decorations as f32,
            output,
            &programs,
        ) {
            elements.push(border.into());
        }
        let clip = framebuffer_clip_rect(
            corners.rect,
            output.current_mode().unwrap().size,
            output.current_transform().invert(),
        );
        elements.push(
            NativeTextureElement {
                id: window.snapshot.id.clone(),
                commit: window.snapshot.commit,
                texture: window.snapshot.texture.clone(),
                geometry: corners.rect,
                source: window.source,
                alpha,
                program: Some(programs.texture.clone()),
                uniforms: vec![Uniform::new("clip_rect", clip), Uniform::new("radius", corners.radius)],
            }
            .into(),
        );
        if let Some(mut fill) = window.fill {
            fill[3] = alpha;
            if let Some(fill) = window_tint_element(
                &mut state.render,
                renderer,
                window.id,
                constrain,
                corners,
                fill,
                true,
                output,
            ) {
                elements.push(fill.into());
            }
        }
        if let Some((surface, role, generation, opacity)) = &window.material {
            if let Some((material, _)) = material_element_with_role(
                state,
                renderer,
                output,
                surface,
                MaterialSurface {
                    geometry: constrain,
                    corners,
                    index: 0,
                    capture_geometry: constrain,
                    alpha,
                },
                (*role, *generation, *opacity),
            ) {
                elements.push(material);
            }
        }
        let theme = &state.theme_settings;
        if let Some(shadow) = window_shadow_element(
            &mut state.render,
            renderer,
            window.id,
            constrain,
            corners,
            scale,
            theme.shadow_offset_y,
            theme.shadow_blur,
            theme.shadow_opacity * f64::from(alpha) * window.decorations,
            theme.shadow_color.0,
            output,
            &programs,
        ) {
            elements.push(shadow.into());
        }
    }
    elements
}
