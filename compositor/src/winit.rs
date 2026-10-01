use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;
use std::time::{Duration, Instant};

use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::RenderElementStates;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Frame, ImportDma, Renderer};
use smithay::backend::winit::{self, WinitEvent};
use smithay::desktop::layer_map_for_output;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind as PresentationKind;
use smithay::utils::{Clock, Monotonic, Physical, Rectangle, Transform};
use smithay::wayland::presentation::Refresh;

use crate::Ferese;
use crate::metrics::{FrameEffectMetrics, RenderMetrics};
use crate::render::{animated_window_elements, frame_effect_metrics, layer_surfaces, output_elements, redraw_output};

pub(crate) type NestedBackend = Rc<RefCell<winit::WinitGraphicsBackend<GlesRenderer>>>;
type DamageRenderResult = Result<
    (
        Option<Vec<Rectangle<i32, Physical>>>,
        FrameEffectMetrics,
        RenderElementStates,
    ),
    Box<dyn Error>,
>;

pub fn init(event_loop: &mut EventLoop<Ferese>, state: &mut Ferese) -> Result<(), Box<dyn Error>> {
    let (mut backend, event_source) = winit::init::<GlesRenderer>()?;
    let dmabuf_formats = backend.renderer().dmabuf_formats();
    let display_handle = state.display_handle.clone();
    state
        .dmabuf_state
        .create_global::<Ferese>(&display_handle, dmabuf_formats);
    let initial_scale = normalized_scale(backend.scale_factor());
    let mode = Mode {
        size: backend.window_size(),
        refresh: backend
            .window()
            .current_monitor()
            .and_then(|monitor| monitor.refresh_rate_millihertz())
            .unwrap_or(60_000) as i32,
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
    state.register_output(&output, "nested-primary".to_owned());

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    let clock = Clock::<Monotonic>::new();
    let mut sequence = 0_u64;
    let mut missed_deadlines = 0_u64;
    let mut render_metrics = RenderMetrics::from_environment(output.name());

    // Do not request a redraw recursively: a no-damage redraw has no EGL
    // submission to pace it and otherwise spins at full CPU. Independently
    // scheduled frames also let clients receive callbacks without damage.
    let refresh = Rc::new(Cell::new(Duration::from_nanos(
        1_000_000_000_000 / mode.refresh.max(1) as u64,
    )));
    let backend = Rc::new(RefCell::new(backend));
    state.nested_backend = Some(backend.clone());
    let redraw_backend = backend.clone();
    let redraw_refresh = refresh.clone();
    event_loop
        .handle()
        .insert_source(Timer::from_duration(refresh.get()), move |_, _, _| {
            redraw_backend.borrow().window().request_redraw();
            TimeoutAction::ToDuration(redraw_refresh.get())
        })?;

    event_loop
        .handle()
        .insert_source(event_source, move |event, _, state| {
            let mut backend = backend.borrow_mut();
            match event {
                WinitEvent::Resized { size, scale_factor } => {
                    state.backdrop_generation = state.backdrop_generation.wrapping_add(1);
                    let scale = normalized_scale(scale_factor);
                    let rate = backend
                        .window()
                        .current_monitor()
                        .and_then(|monitor| monitor.refresh_rate_millihertz())
                        .unwrap_or(60_000) as i32;
                    refresh.set(Duration::from_nanos(1_000_000_000_000 / rate.max(1) as u64));

                    output.change_current_state(
                        Some(Mode { size, refresh: rate }),
                        None,
                        Some(Scale::Fractional(scale)),
                        None,
                    );

                    state.relayout();
                    if let Err(error) = state.display_handle.flush_clients() {
                        tracing::debug!(%error, "failed to flush output-resize configure");
                    }
                    backend.window().request_redraw();
                }
                WinitEvent::Input(event) => state.process_input_event(event),
                WinitEvent::Redraw => {
                    state.advance_animations(Instant::now());
                    let age = backend.buffer_age().unwrap_or(0);
                    let render_started = Instant::now();
                    let rendered = (|| -> DamageRenderResult {
                        {
                            let (renderer, mut framebuffer) = backend.bind()?;
                            state.process_dmabuf_imports(renderer, None);
                            let elements = animated_window_elements(state, renderer, &output);
                            let effects = frame_effect_metrics(&elements, output.current_scale().fractional_scale());
                            let result = damage_tracker.render_output(
                                renderer,
                                &mut framebuffer,
                                age,
                                &elements,
                                [0.035, 0.04, 0.055, 1.0],
                            )?;
                            let cursorless_capture = state.has_pending_screencopy(&output, false);
                            let captured_with_cursor =
                                state.process_screencopies(renderer, &framebuffer, &output, true);

                            if cursorless_capture {
                                let cursorless_elements = output_elements(state, renderer, &output, false);
                                redraw_output(renderer, &mut framebuffer, &output, &cursorless_elements)?;
                                state.process_screencopies(renderer, &framebuffer, &output, false);
                                redraw_output(renderer, &mut framebuffer, &output, &elements)?;
                            } else if captured_with_cursor {
                                let _ = renderer
                                    .render(
                                        &mut framebuffer,
                                        output.current_mode().expect("output has a mode").size,
                                        output.current_transform(),
                                    )?
                                    .finish()?;
                            }

                            Ok((result.damage.cloned(), effects, result.states))
                        }
                    })();
                    let (damage, effects, rendered_states) = match rendered {
                        Ok((Some(damage), effects, states)) => (damage, effects, states),
                        Ok((None, _, _)) => {
                            render_metrics.record_no_damage(render_started.elapsed(), missed_deadlines);
                            // A callback means permission to draw the next client
                            // frame, not proof of a new compositor presentation.
                            // No-damage frames must still unblock layer clients.
                            send_nested_frame_callbacks(state, &output, refresh.get());
                            state.space.refresh();
                            state.popups.cleanup();
                            layer_map_for_output(&output).cleanup();
                            if let Err(error) = state.display_handle.flush_clients() {
                                tracing::debug!(%error, "failed to flush clients");
                            }
                            return;
                        }
                        Err(error) => {
                            tracing::error!(%error, "nested renderer failed");
                            state.loop_signal.stop();
                            return;
                        }
                    };
                    if let Err(error) = backend.submit(Some(&damage)) {
                        tracing::error!(%error, "nested buffer submission failed");
                        state.loop_signal.stop();
                        return;
                    }
                    if state.session_lock.active {
                        state.lock_frame_presented(&output);
                    }
                    let elapsed = render_started.elapsed();
                    if elapsed > refresh.get() {
                        missed_deadlines += (elapsed.as_nanos() / refresh.get().as_nanos()) as u64;
                    }
                    render_metrics.record_frame(elapsed, &damage, missed_deadlines, effects);

                    let mut presentation = crate::presentation::take_output_feedback(
                        state,
                        &output,
                        &rendered_states,
                        PresentationKind::Vsync,
                    );
                    sequence = sequence.wrapping_add(1);
                    presentation.presented(
                        clock.now(),
                        Refresh::fixed(refresh.get()),
                        sequence,
                        PresentationKind::Vsync,
                    );
                    send_nested_frame_callbacks(state, &output, refresh.get());
                    state.space.refresh();
                    state.popups.cleanup();
                    layer_map_for_output(&output).cleanup();
                    if let Err(error) = state.display_handle.flush_clients() {
                        tracing::debug!(%error, "failed to flush clients");
                    }
                }
                WinitEvent::CloseRequested => state.loop_signal.stop(),
                _ => {}
            }
        })?;
    Ok(())
}

fn send_nested_frame_callbacks(state: &mut Ferese, output: &Output, refresh: Duration) {
    if state.session_lock.active {
        state.lock_frame_callbacks(output);
        return;
    }
    let time = state.start_time.elapsed();
    for window in state.space.elements().filter(|window| {
        state
            .windows
            .ids()
            .get(*window)
            .is_some_and(|id| state.window_belongs_to_output(*id, output))
    }) {
        window.send_frame(output, time, Some(refresh), |_, _| Some(output.clone()));
    }
    for layer in layer_surfaces(output) {
        layer.send_frame(output, time, Some(refresh), |_, _| Some(output.clone()));
    }
    state.send_cursor_frame(output);
}

fn normalized_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 { scale } else { 1.0 }
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
