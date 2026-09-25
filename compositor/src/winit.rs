use std::{
    collections::HashMap,
    error::Error,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        renderer::{
            Color32F, ErasedContextId, Frame, ImportDma, Renderer,
            damage::OutputDamageTracker,
            element::{
                AsRenderElements, Element, Id, Kind as RenderElementKind, RenderElement,
                UnderlyingStorage,
                memory::MemoryRenderBufferRenderElement,
                render_elements,
                surface::{
                    WaylandSurfaceRenderElement, WaylandSurfaceTexture,
                    render_elements_from_surface_tree,
                },
                utils::{
                    ConstrainAlign, ConstrainScaleBehavior, CropRenderElement, Relocate,
                    RelocateRenderElement, RescaleRenderElement, constrain_render_elements,
                },
            },
            gles::{
                GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, Uniform,
                UniformName, UniformType, element::PixelShaderElement,
            },
            utils::{CommitCounter, DamageSet, OpaqueRegions, draw_render_elements},
        },
        winit::{self, WinitEvent},
    },
    desktop::{
        LayerSurface, PopupManager, layer_map_for_output,
        space::{ConstrainBehavior, ConstrainReference, constrain_space_element},
        utils::OutputPresentationFeedback,
    },
    input::pointer::{CursorImageStatus, CursorImageSurfaceData},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::calloop::EventLoop,
    reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind as PresentationKind,
    utils::{
        Buffer, Clock, Logical, Monotonic, Physical, Point, Rectangle, Scale as RenderScale,
        Transform,
    },
    wayland::shell::wlr_layer::Layer,
    wayland::{compositor::with_states, presentation::Refresh},
};

use crate::Ferese;

type SurfaceRenderElement = CropRenderElement<
    RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
>;

type WindowRenderElement =
    CropRenderElement<RelocateRenderElement<RescaleRenderElement<WindowContentRenderElement>>>;

type MemoryRenderElement = CropRenderElement<
    RelocateRenderElement<RescaleRenderElement<MemoryRenderBufferRenderElement<GlesRenderer>>>,
>;

render_elements! {
    pub(crate) AnimatedWindowRenderElement<=GlesRenderer>;
    Window=WindowRenderElement,
    Surface=SurfaceRenderElement,
    Memory=MemoryRenderElement,
    Border=PixelShaderElement,
}

render_elements! {
    WindowContentRenderElement<=GlesRenderer>;
    Rounded=RoundedSurfaceRenderElement,
    Popup=WaylandSurfaceRenderElement<GlesRenderer>,
}

const ROUNDED_TEXTURE_SHADER: &str = r#"#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
uniform vec4 clip_rect;
uniform float radius;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

void main() {
    vec4 color = texture2D(tex, v_coords);

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif

    vec2 point = gl_FragCoord.xy - clip_rect.xy;
    vec2 half_size = clip_rect.zw * 0.5;
    vec2 distance = abs(point - half_size) - (half_size - vec2(radius));
    float signed_distance = length(max(distance, 0.0))
        + min(max(distance.x, distance.y), 0.0)
        - radius;
    float coverage = 1.0 - smoothstep(-0.5, 0.5, signed_distance);
    color *= coverage;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

const ROUNDED_BORDER_SHADER: &str = r#"
precision mediump float;

uniform float alpha;
uniform vec4 clip_rect;
uniform float radius;
uniform float border_width;
uniform vec4 border_color;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

void main() {
    vec2 point = gl_FragCoord.xy - clip_rect.xy;
    vec2 half_size = clip_rect.zw * 0.5;
    vec2 distance = abs(point - half_size) - (half_size - vec2(radius));
    float signed_distance = length(max(distance, 0.0))
        + min(max(distance.x, distance.y), 0.0)
        - radius;
    float outer_coverage = 1.0 - smoothstep(-0.5, 0.5, signed_distance);

    vec2 inner_half_size = max(half_size - vec2(border_width), vec2(0.0));
    float inner_radius = max(radius - border_width, 0.0);
    vec2 inner_distance = abs(point - half_size)
        - (inner_half_size - vec2(inner_radius));
    float inner_signed_distance = length(max(inner_distance, 0.0))
        + min(max(inner_distance.x, inner_distance.y), 0.0)
        - inner_radius;
    float inner_coverage = 1.0 - smoothstep(-0.5, 0.5, inner_signed_distance);
    float coverage = max(outer_coverage - inner_coverage, 0.0);
    vec4 color = vec4(border_color.rgb * border_color.a, border_color.a)
        * coverage * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

#[derive(Clone, Debug)]
pub(crate) struct RoundedClipPrograms {
    texture: GlesTexProgram,
    border: GlesPixelProgram,
}

#[derive(Clone, Debug, PartialEq)]
struct BorderParameters {
    geometry: Rectangle<i32, Logical>,
    clip_rect: [f32; 4],
    radius: f32,
    width: f32,
    color: [f32; 4],
}

#[derive(Debug)]
struct CachedBorder {
    element: PixelShaderElement,
    parameters: BorderParameters,
}

#[derive(Debug, Default)]
pub(crate) struct WindowBorderBuffers {
    contexts: HashMap<ErasedContextId, CachedBorder>,
}

#[derive(Debug)]
struct RoundedSurfaceRenderElement {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    program: GlesTexProgram,
    clip_rect: [f32; 4],
    radius: f32,
}

impl Element for RoundedSurfaceRenderElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn geometry(&self, scale: RenderScale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn damage_since(
        &self,
        scale: RenderScale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, _scale: RenderScale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> RenderElementKind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for RoundedSurfaceRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        match self.inner.texture() {
            WaylandSurfaceTexture::Texture(texture) => frame.render_texture_from_to(
                texture,
                src,
                dst,
                damage,
                opaque_regions,
                self.transform(),
                self.alpha(),
                Some(&self.program),
                &[
                    Uniform::new("clip_rect", self.clip_rect),
                    Uniform::new("radius", self.radius),
                ],
            ),
            WaylandSurfaceTexture::SolidColor(_) => {
                self.inner.draw(frame, src, dst, damage, opaque_regions)
            }
        }
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        self.inner.underlying_storage(renderer)
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
    let rounded_clip_program = rounded_clip_program(state, renderer);

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

        if !fullscreen && let Some(programs) = rounded_clip_program.clone() {
            let focused = state.focused_window == Some(id);
            let border_width = if focused {
                state.theme_settings.focus_ring_width
            } else {
                state.theme_settings.border_width
            };
            let border_color = if focused {
                state.theme_settings.accent_color.0
            } else {
                state.theme_settings.border_color.0
            };

            if let Some(border) = window_border_element(
                state,
                renderer,
                id,
                constrain,
                scale,
                state.theme_settings.window_radius,
                border_width,
                border_color,
                output,
                &programs,
            ) {
                elements.push(border.into());
            }
            elements.extend(rounded_window_elements(
                renderer,
                &window,
                constrain,
                scale,
                state.theme_settings.window_radius,
                output,
                programs.texture,
            ));
        } else {
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
    }
    elements.extend(layer_elements(
        renderer,
        output,
        &[Layer::Bottom, Layer::Background],
    ));
    elements
}

fn rounded_clip_program(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
) -> Option<RoundedClipPrograms> {
    let context = renderer.context_id().erased();
    if let Some(program) = state.rounded_clip_programs.get(&context) {
        return Some(program.clone());
    }

    let texture_uniforms = [
        UniformName::new("clip_rect", UniformType::_4f),
        UniformName::new("radius", UniformType::_1f),
    ];
    let border_uniforms = [
        UniformName::new("clip_rect", UniformType::_4f),
        UniformName::new("radius", UniformType::_1f),
        UniformName::new("border_width", UniformType::_1f),
        UniformName::new("border_color", UniformType::_4f),
    ];
    let texture = renderer.compile_custom_texture_shader(ROUNDED_TEXTURE_SHADER, &texture_uniforms);
    let border = renderer.compile_custom_pixel_shader(ROUNDED_BORDER_SHADER, &border_uniforms);
    match texture.and_then(|texture| border.map(|border| (texture, border))) {
        Ok((texture, border)) => {
            let programs = RoundedClipPrograms { texture, border };
            state
                .rounded_clip_programs
                .insert(context, programs.clone());
            Some(programs)
        }
        Err(error) => {
            tracing::error!(%error, "failed to compile rounded-window shader");
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn window_border_element(
    state: &mut Ferese,
    renderer: &GlesRenderer,
    id: ferese_layout::WindowId,
    geometry: Rectangle<i32, Logical>,
    scale: f64,
    requested_radius: f64,
    requested_width: f64,
    color: [f32; 4],
    output: &Output,
    programs: &RoundedClipPrograms,
) -> Option<PixelShaderElement> {
    let mode = output.current_mode()?;
    let width = scaled_effect_value(requested_width, geometry, scale);
    if width == 0.0 {
        return None;
    }

    let parameters = BorderParameters {
        geometry,
        clip_rect: framebuffer_clip_rect(
            geometry.to_physical_precise_round(scale),
            mode.size,
            output.current_transform().invert(),
        ),
        radius: scaled_effect_value(requested_radius, geometry, scale),
        width,
        color,
    };
    let context = renderer.context_id().erased();
    let buffers = state.window_borders.entry(id).or_default();
    if !buffers.contexts.contains_key(&context) {
        let element = PixelShaderElement::new(
            programs.border.clone(),
            geometry,
            None,
            1.0,
            border_uniforms(&parameters),
            RenderElementKind::Unspecified,
        );
        buffers.contexts.insert(
            context.clone(),
            CachedBorder {
                element,
                parameters: parameters.clone(),
            },
        );
    }

    let cached = buffers.contexts.get_mut(&context)?;
    if cached.parameters != parameters {
        if cached.parameters.geometry != parameters.geometry {
            cached.element.resize(parameters.geometry, None);
        }
        cached.element.update_uniforms(border_uniforms(&parameters));
        cached.parameters = parameters;
    }

    Some(cached.element.clone())
}

fn border_uniforms(parameters: &BorderParameters) -> Vec<Uniform<'static>> {
    vec![
        Uniform::new("clip_rect", parameters.clip_rect).into_owned(),
        Uniform::new("radius", parameters.radius).into_owned(),
        Uniform::new("border_width", parameters.width).into_owned(),
        Uniform::new("border_color", parameters.color).into_owned(),
    ]
}

fn scaled_effect_value(requested: f64, geometry: Rectangle<i32, Logical>, scale: f64) -> f32 {
    requested
        .min(f64::from(geometry.size.w.min(geometry.size.h)) / 2.0)
        .max(0.0) as f32
        * scale as f32
}

#[allow(clippy::too_many_arguments)]
fn rounded_window_elements(
    renderer: &mut GlesRenderer,
    window: &smithay::desktop::Window,
    constrain: Rectangle<i32, Logical>,
    scale: f64,
    requested_radius: f64,
    output: &Output,
    program: GlesTexProgram,
) -> Vec<AnimatedWindowRenderElement> {
    let Some(toplevel) = window.toplevel() else {
        return Vec::new();
    };
    let Some(mode) = output.current_mode() else {
        return Vec::new();
    };

    let geometry = window.geometry();
    let reference = geometry.to_physical_precise_round(scale);
    let physical_constrain = constrain.to_physical_precise_round(scale);
    let location = (constrain.loc - geometry.loc).to_physical_precise_round(scale);
    let clip = framebuffer_clip_rect(
        physical_constrain,
        mode.size,
        output.current_transform().invert(),
    );
    let radius = scaled_effect_value(requested_radius, constrain, scale);
    let surface = toplevel.wl_surface();

    let mut content = PopupManager::popups_for_surface(surface)
        .flat_map(|(popup, popup_offset)| {
            let offset = (geometry.loc + popup_offset - popup.geometry().loc)
                .to_physical_precise_round(scale);

            render_elements_from_surface_tree::<
                GlesRenderer,
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(
                renderer,
                popup.wl_surface(),
                location + offset,
                scale,
                1.0,
                RenderElementKind::Unspecified,
            )
            .into_iter()
            .map(WindowContentRenderElement::from)
        })
        .collect::<Vec<_>>();
    content.extend(
        render_elements_from_surface_tree::<
            GlesRenderer,
            WaylandSurfaceRenderElement<GlesRenderer>,
        >(
            renderer,
            surface,
            location,
            scale,
            1.0,
            RenderElementKind::Unspecified,
        )
        .into_iter()
        .map(|inner| {
            RoundedSurfaceRenderElement {
                inner,
                program: program.clone(),
                clip_rect: clip,
                radius,
            }
            .into()
        }),
    );

    constrain_render_elements(
        content,
        location,
        physical_constrain,
        reference,
        ConstrainScaleBehavior::Stretch,
        ConstrainAlign::TOP | ConstrainAlign::LEFT,
        scale,
    )
    .map(Into::into)
    .collect()
}

fn framebuffer_clip_rect(
    geometry: Rectangle<i32, Physical>,
    output_size: smithay::utils::Size<i32, Physical>,
    transform: Transform,
) -> [f32; 4] {
    let transformed = transform.transform_rect_in(geometry, &output_size);
    let transformed_output_size = transform.transform_size(output_size);
    let bottom = transformed_output_size.h - transformed.loc.y - transformed.size.h;

    [
        transformed.loc.x as f32,
        bottom as f32,
        transformed.size.w as f32,
        transformed.size.h as f32,
    ]
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
    use smithay::utils::{Physical, Rectangle, Transform};

    use super::{framebuffer_clip_rect, normalized_scale};

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
    fn clip_rect_uses_opengl_bottom_left_origin() {
        let geometry = Rectangle::<i32, Physical>::new((10, 5).into(), (30, 40).into());

        assert_eq!(
            framebuffer_clip_rect(geometry, (100, 80).into(), Transform::Normal),
            [10.0, 35.0, 30.0, 40.0]
        );
    }

    #[test]
    fn clip_rect_follows_output_transform() {
        let geometry = Rectangle::<i32, Physical>::new((10, 5).into(), (30, 40).into());

        assert_eq!(
            framebuffer_clip_rect(geometry, (100, 80).into(), Transform::Flipped180),
            [10.0, 5.0, 30.0, 40.0]
        );
        assert_eq!(
            framebuffer_clip_rect(geometry, (100, 80).into(), Transform::_180),
            [60.0, 5.0, 30.0, 40.0]
        );
    }
}
