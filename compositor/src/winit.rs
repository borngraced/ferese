use std::{
    error::Error,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        renderer::{
            Color32F, Frame, ImportDma, Renderer,
            damage::OutputDamageTracker,
            element::{
                AsRenderElements, Kind as RenderElementKind, RenderElement,
                memory::MemoryRenderBufferRenderElement,
                render_elements,
                solid::{SolidColorBuffer, SolidColorRenderElement},
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
                utils::{
                    ConstrainAlign, ConstrainScaleBehavior, CropRenderElement, Relocate,
                    RelocateRenderElement, RescaleRenderElement,
                },
            },
            gles::GlesRenderer,
            utils::draw_render_elements,
        },
        winit::{self, WinitEvent},
    },
    desktop::{
        LayerSurface, layer_map_for_output,
        space::{ConstrainBehavior, ConstrainReference, constrain_space_element},
        utils::OutputPresentationFeedback,
    },
    input::pointer::{CursorImageStatus, CursorImageSurfaceData},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::calloop::EventLoop,
    reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind as PresentationKind,
    utils::{Clock, Logical, Monotonic, Physical, Point, Rectangle, Transform},
    wayland::shell::wlr_layer::Layer,
    wayland::{compositor::with_states, presentation::Refresh},
};

use crate::Ferese;

type SurfaceRenderElement = CropRenderElement<
    RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
>;

type MemoryRenderElement = CropRenderElement<
    RelocateRenderElement<RescaleRenderElement<MemoryRenderBufferRenderElement<GlesRenderer>>>,
>;

render_elements! {
    pub(crate) AnimatedWindowRenderElement<=GlesRenderer>;
    Surface=SurfaceRenderElement,
    Memory=MemoryRenderElement,
    Solid=SolidColorRenderElement,
}

#[derive(Debug)]
pub(crate) struct WindowBorderBuffers {
    edges: [SolidColorBuffer; 4],
}

impl Default for WindowBorderBuffers {
    fn default() -> Self {
        Self {
            edges: std::array::from_fn(|_| SolidColorBuffer::default()),
        }
    }
}

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
    state.register_output(&output, "nested-primary".to_owned());

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
                        |_, _| PresentationKind::Vsync,
                    );
                });
                layer_surfaces(&output).iter().for_each(|layer| {
                    layer.take_presentation_feedback(
                        &mut presentation,
                        |_, _| Some(output.clone()),
                        |_, _| PresentationKind::Vsync,
                    );
                });
                let rendered = (|| -> Result<(), Box<dyn Error>> {
                    {
                        let (renderer, mut framebuffer) = backend.bind()?;
                        state.process_dmabuf_imports(renderer);
                        let elements = animated_window_elements(state, renderer, &output);
                        damage_tracker.render_output(
                            renderer,
                            &mut framebuffer,
                            0,
                            &elements,
                            [0.035, 0.04, 0.055, 1.0],
                        )?;
                        let cursorless_capture = state.has_pending_screencopy(&output, false);
                        let captured_with_cursor =
                            state.process_screencopies(renderer, &framebuffer, &output, true);

                        if cursorless_capture {
                            let cursorless_elements =
                                output_elements(state, renderer, &output, false);
                            redraw_output(
                                renderer,
                                &mut framebuffer,
                                &output,
                                &cursorless_elements,
                            )?;
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
                    PresentationKind::Vsync,
                );
                state.space.elements().for_each(|window| {
                    window.send_frame(
                        &output,
                        state.start_time.elapsed(),
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    );
                });
                layer_surfaces(&output).iter().for_each(|layer| {
                    layer.send_frame(
                        &output,
                        state.start_time.elapsed(),
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    );
                });
                state.send_cursor_frame(&output);
                state.space.refresh();
                state.popups.cleanup();
                layer_map_for_output(&output).cleanup();
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
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
) -> Vec<AnimatedWindowRenderElement> {
    output_elements(state, renderer, output, true)
}

pub(crate) fn cursorless_window_elements(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
) -> Vec<AnimatedWindowRenderElement> {
    output_elements(state, renderer, output, false)
}

fn output_elements(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
    include_cursor: bool,
) -> Vec<AnimatedWindowRenderElement> {
    let Some(output_geometry) = state.space.output_geometry(output) else {
        return Vec::new();
    };
    let scale = output.current_scale().fractional_scale();

    let mut elements = if include_cursor {
        cursor_elements(state, renderer, output_geometry, scale)
    } else {
        Vec::new()
    };
    elements.extend(layer_elements(
        renderer,
        output,
        &[Layer::Overlay, Layer::Top],
    ));
    let windows = state
        .space
        .elements()
        .rev()
        .filter_map(|window| {
            let id = *state.window_ids.get(window)?;
            let visual = state.window_geometry.get(&id)?.visual.current;
            let fullscreen = state
                .workspaces
                .workspace_for_window(id)
                .and_then(|workspace| state.workspaces.workspace(workspace))
                .is_some_and(|workspace| workspace.fullscreen == Some(id));

            Some((window.clone(), id, visual, fullscreen))
        })
        .collect::<Vec<_>>();

    for (window, id, visual, fullscreen) in windows {
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

        if !fullscreen {
            elements.extend(window_border_elements(state, id, constrain, scale));
        }
        elements.extend(constrain_space_element::<
            GlesRenderer,
            _,
            AnimatedWindowRenderElement,
        >(
            renderer,
            &window,
            constrain.loc,
            1.0,
            scale,
            constrain,
            ConstrainBehavior {
                reference: ConstrainReference::Geometry,
                behavior: ConstrainScaleBehavior::Stretch,
                align: ConstrainAlign::TOP | ConstrainAlign::LEFT,
            },
        ));
    }
    elements.extend(layer_elements(
        renderer,
        output,
        &[Layer::Bottom, Layer::Background],
    ));
    elements
}

fn window_border_elements(
    state: &mut Ferese,
    id: ferese_layout::WindowId,
    geometry: Rectangle<i32, Logical>,
    scale: f64,
) -> Vec<AnimatedWindowRenderElement> {
    let focused = state.focused_window == Some(id);
    let width = if focused {
        state.theme_settings.focus_ring_width
    } else {
        state.theme_settings.border_width
    };
    let rectangles = border_rectangles(geometry, width);
    if rectangles.is_empty() {
        return Vec::new();
    }

    let color = if focused {
        state.theme_settings.accent_color.0
    } else {
        state.theme_settings.border_color.0
    };
    let buffers = state.window_borders.entry(id).or_default();

    buffers
        .edges
        .iter_mut()
        .zip(rectangles)
        .map(|(buffer, rectangle)| {
            buffer.update(rectangle.size, color);
            SolidColorRenderElement::from_buffer(
                buffer,
                rectangle.loc.to_physical_precise_round(scale),
                scale,
                1.0,
                RenderElementKind::Unspecified,
            )
            .into()
        })
        .collect()
}

fn border_rectangles(
    geometry: Rectangle<i32, Logical>,
    requested_width: f64,
) -> Vec<Rectangle<i32, Logical>> {
    let width = requested_width
        .ceil()
        .max(0.0)
        .min(f64::from(geometry.size.w.min(geometry.size.h)) / 2.0) as i32;
    if width == 0 {
        return Vec::new();
    }

    let inner_height = geometry.size.h - width * 2;
    let mut rectangles = vec![
        Rectangle::new(geometry.loc, (geometry.size.w, width).into()),
        Rectangle::new(
            (geometry.loc.x, geometry.loc.y + geometry.size.h - width).into(),
            (geometry.size.w, width).into(),
        ),
    ];
    if inner_height > 0 {
        rectangles.extend([
            Rectangle::new(
                (geometry.loc.x, geometry.loc.y + width).into(),
                (width, inner_height).into(),
            ),
            Rectangle::new(
                (
                    geometry.loc.x + geometry.size.w - width,
                    geometry.loc.y + width,
                )
                    .into(),
                (width, inner_height).into(),
            ),
        ]);
    }

    rectangles
}

pub(crate) fn redraw_output<R, E>(
    renderer: &mut R,
    framebuffer: &mut R::Framebuffer<'_>,
    output: &Output,
    elements: &[E],
) -> Result<(), R::Error>
where
    R: Renderer,
    R::TextureId: 'static,
    E: RenderElement<R>,
{
    let mode = output.current_mode().expect("output has a mode");
    let transform = output.current_transform().invert();
    let damage = Rectangle::from_size(transform.transform_size(mode.size));
    let mut frame = renderer.render(framebuffer, mode.size, transform)?;

    frame.clear(Color32F::new(0.035, 0.04, 0.055, 1.0), &[damage])?;
    draw_render_elements(
        &mut frame,
        output.current_scale().fractional_scale(),
        elements,
        &[damage],
    )?;
    let _ = frame.finish()?;

    Ok(())
}

fn layer_surfaces(output: &Output) -> Vec<LayerSurface> {
    layer_map_for_output(output).layers().cloned().collect()
}

fn layer_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    requested_layers: &[Layer],
) -> Vec<AnimatedWindowRenderElement> {
    let scale = output.current_scale().fractional_scale();
    let Some(mode) = output.current_mode() else {
        return Vec::new();
    };
    let output_crop = Rectangle::<i32, Physical>::from_size(mode.size);
    let layers = {
        let map = layer_map_for_output(output);

        requested_layers
            .iter()
            .flat_map(|requested| {
                map.layers_on(*requested).rev().filter_map(|layer| {
                    map.layer_geometry(layer)
                        .map(|geometry| (geometry.loc, layer.clone()))
                })
            })
            .collect::<Vec<_>>()
    };

    layers
        .into_iter()
        .flat_map(|(location, layer)| {
            AsRenderElements::<GlesRenderer>::render_elements::<
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(
                &layer,
                renderer,
                location.to_physical_precise_round(scale),
                scale.into(),
                1.0,
            )
        })
        .filter_map(|element| {
            let origin = Point::<i32, Physical>::default();
            let element = RescaleRenderElement::from_element(element, origin, 1.0);
            let element = RelocateRenderElement::from_element(element, origin, Relocate::Relative);

            CropRenderElement::from_element(element, scale, output_crop).map(Into::into)
        })
        .collect()
}

fn cursor_elements(
    state: &Ferese,
    renderer: &mut GlesRenderer,
    output_geometry: Rectangle<i32, Logical>,
    scale: f64,
) -> Vec<AnimatedWindowRenderElement> {
    let Some(pointer) = state.seat.get_pointer() else {
        return Vec::new();
    };
    let output_crop =
        Rectangle::<i32, Logical>::from_size(output_geometry.size).to_physical_precise_round(scale);
    let origin = Point::<i32, Physical>::default();
    let pointer_location = pointer.current_location() - output_geometry.loc.to_f64();
    match &state.cursor_status {
        CursorImageStatus::Surface(surface) => {
            let hotspot = with_states(surface, |states| {
                states
                    .data_map
                    .get::<CursorImageSurfaceData>()
                    .map(|attributes| attributes.lock().unwrap().hotspot)
                    .unwrap_or_default()
            });
            let physical_location =
                (pointer_location - hotspot.to_f64()).to_physical_precise_round(scale);

            render_elements_from_surface_tree::<
                GlesRenderer,
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(
                renderer,
                surface,
                physical_location,
                scale,
                1.0,
                RenderElementKind::Cursor,
            )
            .into_iter()
            .filter_map(|element| {
                let element = RescaleRenderElement::from_element(element, origin, 1.0);
                let element =
                    RelocateRenderElement::from_element(element, origin, Relocate::Relative);

                CropRenderElement::from_element(element, scale, output_crop).map(Into::into)
            })
            .collect()
        }
        CursorImageStatus::Named(icon) => {
            let Some(cursor) = state.named_cursors.get(icon) else {
                return Vec::new();
            };
            let hotspot = Point::<i32, Physical>::from((cursor.hotspot.x, cursor.hotspot.y));
            let physical_location = pointer_location.to_physical_precise_round(scale) - hotspot;
            let Ok(element) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                physical_location.to_f64(),
                &cursor.buffer,
                None,
                None,
                None,
                RenderElementKind::Cursor,
            ) else {
                return Vec::new();
            };

            let element = RescaleRenderElement::from_element(element, origin, 1.0);
            let element = RelocateRenderElement::from_element(element, origin, Relocate::Relative);

            CropRenderElement::from_element(element, scale, output_crop)
                .map(|element| vec![element.into()])
                .unwrap_or_default()
        }
        CursorImageStatus::Hidden => Vec::new(),
    }
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
    use smithay::utils::{Logical, Rectangle};

    use super::{border_rectangles, normalized_scale};

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

    #[test]
    fn border_rectangles_stay_inside_window_geometry() {
        let geometry = Rectangle::<i32, Logical>::new((10, 20).into(), (100, 80).into());

        assert_eq!(
            border_rectangles(geometry, 2.0),
            vec![
                Rectangle::new((10, 20).into(), (100, 2).into()),
                Rectangle::new((10, 98).into(), (100, 2).into()),
                Rectangle::new((10, 22).into(), (2, 76).into()),
                Rectangle::new((108, 22).into(), (2, 76).into()),
            ]
        );
    }

    #[test]
    fn border_width_is_clamped_for_tiny_windows() {
        let geometry = Rectangle::<i32, Logical>::new((0, 0).into(), (3, 2).into());
        let rectangles = border_rectangles(geometry, 20.0);

        assert_eq!(rectangles.len(), 2);
        assert!(
            rectangles
                .iter()
                .all(|rectangle| geometry.contains_rect(*rectangle))
        );
    }
}
