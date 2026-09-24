use std::{
    error::Error,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker,
            element::{
                surface::WaylandSurfaceRenderElement,
                utils::{
                    ConstrainAlign, ConstrainScaleBehavior, CropRenderElement,
                    RelocateRenderElement, RescaleRenderElement,
                },
            },
            gles::GlesRenderer,
        },
        winit::{self, WinitEvent},
    },
    desktop::{
        space::{ConstrainBehavior, ConstrainReference, constrain_space_element},
        utils::OutputPresentationFeedback,
    },
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::calloop::EventLoop,
    reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind,
    utils::{Clock, Logical, Monotonic, Rectangle, Transform},
    wayland::presentation::Refresh,
};

use crate::Ferese;

pub(crate) type AnimatedWindowRenderElement = CropRenderElement<
    RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
>;

pub fn init(event_loop: &mut EventLoop<Ferese>, state: &mut Ferese) -> Result<(), Box<dyn Error>> {
    let (mut backend, event_source) = winit::init()?;
    let initial_scale = normalized_scale(backend.scale_factor());
    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };
    let output = Output::new(
        "ferese-winit".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Ferese".into(),
            model: "Nested".into(),
        },
    );
    output.create_global::<Ferese>(&state.display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(Scale::Fractional(initial_scale)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    let clock = Clock::<Monotonic>::new();
    let mut sequence = 0_u64;
    let mut output_scale = initial_scale;

    event_loop
        .handle()
        .insert_source(event_source, move |event, _, state| match event {
            WinitEvent::Resized { size, scale_factor } => {
                let scale = normalized_scale(scale_factor);

                output.change_current_state(
                    Some(Mode {
                        size,
                        refresh: 60_000,
                    }),
                    None,
                    Some(Scale::Fractional(scale)),
                    None,
                );

                if scale != output_scale {
                    output_scale = scale;
                    state.update_fractional_scale(scale);
                }

                state.relayout();
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                state.advance_animations(Instant::now());
                let damage = Rectangle::from_size(backend.window_size());
                let mut presentation = OutputPresentationFeedback::new(&output);
                state.space.elements().for_each(|window| {
                    window.take_presentation_feedback(
                        &mut presentation,
                        |_, _| Some(output.clone()),
                        |_, _| Kind::Vsync,
                    );
                });
                let rendered = (|| -> Result<(), Box<dyn Error>> {
                    {
                        let (renderer, mut framebuffer) = backend.bind()?;
                        let elements = animated_window_elements(state, renderer, &output);
                        damage_tracker.render_output(
                            renderer,
                            &mut framebuffer,
                            0,
                            &elements,
                            [0.035, 0.04, 0.055, 1.0],
                        )?;
                    }
                    backend.submit(Some(&[damage]))?;
                    Ok(())
                })();
                if let Err(error) = rendered {
                    tracing::error!(%error, "nested renderer failed");
                    state.loop_signal.stop();
                    return;
                }
                sequence = sequence.wrapping_add(1);
                presentation.presented(
                    clock.now(),
                    Refresh::fixed(Duration::from_nanos(1_000_000_000 / 60)),
                    sequence,
                    Kind::Vsync,
                );
                state.space.elements().for_each(|window| {
                    window.send_frame(
                        &output,
                        state.start_time.elapsed(),
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    );
                });
                state.space.refresh();
                state.popups.cleanup();
                if let Err(error) = state.display_handle.flush_clients() {
                    tracing::debug!(%error, "failed to flush clients");
                }
                backend.window().request_redraw();
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => {}
        })?;
    Ok(())
}

pub(crate) fn animated_window_elements(
    state: &Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
) -> Vec<AnimatedWindowRenderElement> {
    let Some(output_geometry) = state.space.output_geometry(output) else {
        return Vec::new();
    };
    let scale = output.current_scale().fractional_scale();

    state
        .space
        .elements()
        .rev()
        .filter_map(|window| {
            let id = state.window_ids.get(window)?;
            let visual = state.window_geometry.get(id)?.visual.current;
            let constrain = Rectangle::<i32, Logical>::new(
                (
                    (visual.x - f64::from(output_geometry.loc.x)).round() as i32,
                    (visual.y - f64::from(output_geometry.loc.y)).round() as i32,
                )
                    .into(),
                (
                    visual.width.round().max(1.0) as i32,
                    visual.height.round().max(1.0) as i32,
                )
                    .into(),
            );

            Some(constrain_space_element::<
                GlesRenderer,
                _,
                AnimatedWindowRenderElement,
            >(
                renderer,
                window,
                constrain.loc,
                1.0,
                scale,
                constrain,
                ConstrainBehavior {
                    reference: ConstrainReference::Geometry,
                    behavior: ConstrainScaleBehavior::Zoom,
                    align: ConstrainAlign::TOP | ConstrainAlign::LEFT,
                },
            ))
        })
        .flatten()
        .collect()
}

fn normalized_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::normalized_scale;

    #[test]
    fn accepts_positive_finite_scale() {
        assert_eq!(normalized_scale(1.25), 1.25);
    }

    #[test]
    fn rejects_invalid_scale() {
        assert_eq!(normalized_scale(0.0), 1.0);
        assert_eq!(normalized_scale(f64::NAN), 1.0);
        assert_eq!(normalized_scale(f64::INFINITY), 1.0);
    }
}
