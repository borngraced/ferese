use std::{
    collections::HashMap,
    error::Error,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Color32F, ErasedContextId, Frame, ImportDma, Offscreen, Renderer, Texture,
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
                GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, GlesTexture,
                Uniform, UniformName, UniformType, element::PixelShaderElement,
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
    reexports::wayland_server::Resource,
    utils::{
        Buffer, Clock, Logical, Monotonic, Physical, Point, Rectangle, Scale as RenderScale,
        Transform,
    },
    wayland::shell::wlr_layer::Layer,
    wayland::{compositor::with_states, presentation::Refresh},
};

use crate::{Ferese, metrics::RenderMetrics};

type SurfaceRenderElement = CropRenderElement<
    RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
>;

type WindowRenderElement =
    CropRenderElement<RelocateRenderElement<RescaleRenderElement<WindowContentRenderElement>>>;

type MemoryRenderElement = CropRenderElement<
    RelocateRenderElement<RescaleRenderElement<MemoryRenderBufferRenderElement<GlesRenderer>>>,
>;

type PhysicalDamage = Vec<Rectangle<i32, Physical>>;
type DamageRenderResult = Result<Option<PhysicalDamage>, Box<dyn Error>>;

render_elements! {
    pub(crate) AnimatedWindowRenderElement<=GlesRenderer>;
    Window=WindowRenderElement,
    Surface=SurfaceRenderElement,
    Memory=MemoryRenderElement,
    Border=PixelShaderElement,
    Backdrop=BackdropRenderElement,
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

const WINDOW_SHADOW_SHADER: &str = r#"
precision mediump float;

uniform float alpha;
uniform vec4 shadow_rect;
uniform float radius;
uniform float blur;
uniform float opacity;
uniform vec4 shadow_color;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

void main() {
    vec2 point = gl_FragCoord.xy - shadow_rect.xy;
    vec2 half_size = shadow_rect.zw * 0.5;
    vec2 distance = abs(point - half_size) - (half_size - vec2(radius));
    float signed_distance = length(max(distance, 0.0))
        + min(max(distance.x, distance.y), 0.0)
        - radius;
    float sigma = max(blur * 0.5, 0.5);
    float normalized_distance = max(signed_distance, 0.0) / sigma;
    float coverage = exp(-0.5 * normalized_distance * normalized_distance);
    vec4 color = vec4(shadow_color.rgb * shadow_color.a, shadow_color.a)
        * coverage * opacity * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

const MATERIAL_SHADER: &str = r#"
precision mediump float;

uniform float alpha;
uniform vec4 tint;
uniform float noise_amount;
varying vec2 v_coords;

float random(vec2 point) {
    return fract(sin(dot(point, vec2(12.9898, 78.233))) * 43758.5453);
}

void main() {
    float grain = (random(gl_FragCoord.xy) - 0.5) * noise_amount;
    vec3 color = clamp(tint.rgb + vec3(grain), 0.0, 1.0);
    float opacity = tint.a * alpha;
    gl_FragColor = vec4(color * opacity, opacity);
}
"#;

const BACKDROP_SHADER: &str = r#"#version 100

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
uniform vec4 visible_rect;
uniform vec2 texture_size;
uniform float blur_radius;
uniform float saturation;
uniform float brightness;
uniform vec4 tint;
uniform float noise_amount;
varying vec2 v_coords;

float random(vec2 point) {
    return fract(sin(dot(point, vec2(12.9898, 78.233))) * 43758.5453);
}

float kernel_weight(int tap) {
    int distance = tap < 0 ? -tap : tap;
    if (distance == 0) {
        return 6.0;
    }
    if (distance == 1) {
        return 4.0;
    }
    return 1.0;
}

void main() {
    vec2 point = gl_FragCoord.xy;
    vec2 visible_max = visible_rect.xy + visible_rect.zw;
    if (point.x < visible_rect.x || point.y < visible_rect.y
            || point.x >= visible_max.x || point.y >= visible_max.y) {
        gl_FragColor = vec4(0.0);
        return;
    }

    vec2 step_size = vec2(blur_radius * 0.5) / max(texture_size, vec2(1.0));
    vec4 color = vec4(0.0);
    for (int y = -2; y <= 2; y++) {
        for (int x = -2; x <= 2; x++) {
            float weight = kernel_weight(x) * kernel_weight(y);
            vec2 offset = vec2(float(x), float(y)) * step_size;
            color += texture2D(tex, v_coords + offset) * weight;
        }
    }
    color /= 256.0;

    float luma = dot(color.rgb, vec3(0.2126, 0.7152, 0.0722));
    vec3 adjusted = mix(vec3(luma), color.rgb, saturation) * brightness;
    adjusted = mix(adjusted, tint.rgb, tint.a);
    adjusted += vec3((random(point) - 0.5) * noise_amount);
    gl_FragColor = vec4(clamp(adjusted, 0.0, 1.0) * alpha, alpha);
}
"#;

#[derive(Clone, Debug)]
pub(crate) struct RoundedClipPrograms {
    texture: GlesTexProgram,
    border: GlesPixelProgram,
    shadow: GlesPixelProgram,
}

#[derive(Clone, Debug)]
pub(crate) struct MaterialProgram(GlesPixelProgram);

#[derive(Clone, Debug)]
pub(crate) struct BackdropProgram(GlesTexProgram);

#[derive(Clone, Debug, PartialEq)]
struct MaterialParameters {
    geometry: Rectangle<i32, Logical>,
    tint: [f32; 4],
    noise: f32,
    generation: u64,
    scene_generation: u64,
    opaque: bool,
    blur: f32,
    saturation: f32,
    brightness: f32,
    sample_geometry: Rectangle<i32, Logical>,
    sample_physical: Rectangle<i32, Physical>,
    visible_framebuffer: [f32; 4],
    sample_framebuffer: [f32; 4],
}

#[derive(Debug)]
struct CachedMaterial {
    element: CachedMaterialElement,
    parameters: MaterialParameters,
}

#[derive(Clone, Debug)]
enum CachedMaterialElement {
    Tint(PixelShaderElement),
    Backdrop(BackdropRenderElement),
}

#[derive(Debug, Default)]
pub(crate) struct MaterialBuffers {
    contexts: HashMap<ErasedContextId, CachedMaterial>,
}

#[derive(Clone, Debug)]
struct BackdropRenderElement {
    texture: GlesTexture,
    program: GlesTexProgram,
    id: Id,
    commit: CommitCounter,
    geometry: Rectangle<i32, Logical>,
    capture_rect: [i32; 4],
    uniforms: Vec<Uniform<'static>>,
}

impl BackdropRenderElement {
    fn new(texture: GlesTexture, program: GlesTexProgram, parameters: &MaterialParameters) -> Self {
        Self {
            texture,
            program,
            id: Id::new(),
            commit: CommitCounter::default(),
            geometry: parameters.sample_geometry,
            capture_rect: framebuffer_capture_rect(parameters.sample_framebuffer),
            uniforms: backdrop_uniforms(parameters),
        }
    }

    fn update(&mut self, parameters: &MaterialParameters) {
        self.geometry = parameters.sample_geometry;
        self.capture_rect = framebuffer_capture_rect(parameters.sample_framebuffer);
        self.uniforms = backdrop_uniforms(parameters);
        self.commit.increment();
    }
}

impl Element for BackdropRenderElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(self.texture.size().to_f64())
    }

    fn geometry(&self, scale: RenderScale<f64>) -> Rectangle<i32, Physical> {
        self.geometry.to_physical_precise_round(scale)
    }

    fn damage_since(
        &self,
        scale: RenderScale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        if commit == Some(self.commit) {
            DamageSet::default()
        } else {
            DamageSet::from_slice(&[Rectangle::from_size(self.geometry(scale).size)])
        }
    }

    fn opaque_regions(&self, _scale: RenderScale<f64>) -> OpaqueRegions<i32, Physical> {
        // Backdrop dependencies must remain visible to damage tracking even
        // though the shader produces opaque pixels inside the visible region.
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        1.0
    }

    fn kind(&self) -> RenderElementKind {
        RenderElementKind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for BackdropRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let texture = self.texture.tex_id();
        let [x, y, width, height] = self.capture_rect;
        frame.with_context(|gl| unsafe {
            gl.BindTexture(smithay::backend::renderer::gles::ffi::TEXTURE_2D, texture);
            gl.TexParameteri(
                smithay::backend::renderer::gles::ffi::TEXTURE_2D,
                smithay::backend::renderer::gles::ffi::TEXTURE_WRAP_S,
                smithay::backend::renderer::gles::ffi::CLAMP_TO_EDGE as i32,
            );
            gl.TexParameteri(
                smithay::backend::renderer::gles::ffi::TEXTURE_2D,
                smithay::backend::renderer::gles::ffi::TEXTURE_WRAP_T,
                smithay::backend::renderer::gles::ffi::CLAMP_TO_EDGE as i32,
            );
            gl.CopyTexSubImage2D(
                smithay::backend::renderer::gles::ffi::TEXTURE_2D,
                0,
                0,
                0,
                x,
                y,
                width,
                height,
            );
            gl.BindTexture(smithay::backend::renderer::gles::ffi::TEXTURE_2D, 0);
        })?;

        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            1.0,
            Some(&self.program),
            &self.uniforms,
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
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

#[derive(Clone, Debug, PartialEq)]
struct ShadowParameters {
    bounds: Rectangle<i32, Logical>,
    shadow_rect: [f32; 4],
    radius: f32,
    blur: f32,
    opacity: f32,
    color: [f32; 4],
}

#[derive(Debug)]
struct CachedShadow {
    element: PixelShaderElement,
    parameters: ShadowParameters,
}

#[derive(Debug, Default)]
pub(crate) struct WindowShadowBuffers {
    contexts: HashMap<ErasedContextId, CachedShadow>,
}

#[derive(Debug)]
struct RoundedSurfaceRenderElement {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    program: GlesTexProgram,
    clip_rect: [f32; 4],
    radius: f32,
    clip_changed: bool,
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
        if self.clip_changed {
            return DamageSet::from_slice(&[Rectangle::from_size(self.inner.geometry(scale).size)]);
        }
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: RenderScale<f64>) -> OpaqueRegions<i32, Physical> {
        if self.radius == 0.0 {
            self.inner.opaque_regions(scale)
        } else {
            OpaqueRegions::default()
        }
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
    let mut render_metrics = RenderMetrics::from_environment(output.name());

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
                let age = backend.buffer_age().unwrap_or(0);
                let render_started = Instant::now();
                let rendered = (|| -> DamageRenderResult {
                    {
                        let (renderer, mut framebuffer) = backend.bind()?;
                        state.process_dmabuf_imports(renderer);
                        let elements = animated_window_elements(state, renderer, &output);
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

                        Ok(result.damage.cloned())
                    }
                })();
                let damage = match rendered {
                    Ok(Some(damage)) => damage,
                    Ok(None) => {
                        state.space.refresh();
                        state.popups.cleanup();
                        layer_map_for_output(&output).cleanup();
                        if let Err(error) = state.display_handle.flush_clients() {
                            tracing::debug!(%error, "failed to flush clients");
                        }
                        backend.window().request_redraw();
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
                render_metrics.record_frame(render_started.elapsed(), &damage, 0);

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
        state,
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
            let (close_scale, close_alpha) = state.closing_visual(id);
            let visual = scaled_visual_rect(visual, close_scale);
            let decoration_progress = state
                .window_geometry
                .get(&id)
                .map_or(1.0, |geometry| geometry.decorations.clamp(0.0, 1.0));

            Some((window.clone(), id, visual, decoration_progress, close_alpha))
        })
        .collect::<Vec<_>>();

    for (window, id, visual, decoration_progress, close_alpha) in windows {
        let constrain = rounded_visual_rect(visual, output_geometry.loc);

        if let Some(programs) = rounded_clip_program.clone() {
            let window_radius = state.theme_settings.window_radius * decoration_progress;
            let shadow_offset_y = state.theme_settings.shadow_offset_y;
            let shadow_blur = state.theme_settings.shadow_blur;
            let shadow_opacity = state.theme_settings.shadow_opacity;
            let shadow_color = state.theme_settings.shadow_color.0;
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
            let border_color =
                color_with_alpha(border_color, close_alpha * decoration_progress as f32);

            if let Some(border) = window_border_element(
                state,
                renderer,
                id,
                constrain,
                scale,
                window_radius,
                border_width,
                border_color,
                output,
                &programs,
            ) {
                elements.push(border.into());
            }
            let shadow = window_shadow_element(
                state,
                renderer,
                id,
                constrain,
                scale,
                window_radius,
                shadow_offset_y,
                shadow_blur,
                shadow_opacity * f64::from(close_alpha) * decoration_progress,
                shadow_color,
                output,
                &programs,
            );
            elements.extend(rounded_window_elements(
                renderer,
                &window,
                constrain,
                scale,
                window_radius,
                close_alpha,
                state
                    .window_geometry
                    .get(&id)
                    .is_some_and(|geometry| geometry.presentation_changed),
                output,
                programs.texture.clone(),
            ));
            if let Some(shadow) = shadow {
                elements.push(shadow.into());
            }
        } else {
            elements.extend(constrain_space_element::<
                GlesRenderer,
                _,
                AnimatedWindowRenderElement,
            >(
                renderer,
                &window,
                constrain.loc,
                close_alpha,
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
        state,
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
    let shadow_uniforms = [
        UniformName::new("shadow_rect", UniformType::_4f),
        UniformName::new("radius", UniformType::_1f),
        UniformName::new("blur", UniformType::_1f),
        UniformName::new("opacity", UniformType::_1f),
        UniformName::new("shadow_color", UniformType::_4f),
    ];
    let texture = renderer.compile_custom_texture_shader(ROUNDED_TEXTURE_SHADER, &texture_uniforms);
    let border = renderer.compile_custom_pixel_shader(ROUNDED_BORDER_SHADER, &border_uniforms);
    let shadow = renderer.compile_custom_pixel_shader(WINDOW_SHADOW_SHADER, &shadow_uniforms);
    let compiled = texture.and_then(|texture| {
        border.and_then(|border| shadow.map(|shadow| (texture, border, shadow)))
    });
    match compiled {
        Ok((texture, border, shadow)) => {
            let programs = RoundedClipPrograms {
                texture,
                border,
                shadow,
            };
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

#[allow(clippy::too_many_arguments)]
fn window_shadow_element(
    state: &mut Ferese,
    renderer: &GlesRenderer,
    id: ferese_layout::WindowId,
    geometry: Rectangle<i32, Logical>,
    scale: f64,
    requested_radius: f64,
    offset_y: f64,
    blur: f64,
    opacity: f64,
    color: [f32; 4],
    output: &Output,
    programs: &RoundedClipPrograms,
) -> Option<PixelShaderElement> {
    let mode = output.current_mode()?;
    if opacity == 0.0 || color[3] == 0.0 {
        return None;
    }

    let shadow_geometry = Rectangle::new(
        (geometry.loc.x, geometry.loc.y + offset_y.round() as i32).into(),
        geometry.size,
    );
    let bounds = shadow_bounds(geometry, offset_y, blur);
    let parameters = ShadowParameters {
        bounds,
        shadow_rect: framebuffer_clip_rect(
            shadow_geometry.to_physical_precise_round(scale),
            mode.size,
            output.current_transform().invert(),
        ),
        radius: scaled_effect_value(requested_radius, geometry, scale),
        blur: (blur * scale) as f32,
        opacity: opacity as f32,
        color,
    };
    let context = renderer.context_id().erased();
    let buffers = state.window_shadows.entry(id).or_default();
    if !buffers.contexts.contains_key(&context) {
        let element = PixelShaderElement::new(
            programs.shadow.clone(),
            bounds,
            None,
            1.0,
            shadow_uniforms(&parameters),
            RenderElementKind::Unspecified,
        );
        buffers.contexts.insert(
            context.clone(),
            CachedShadow {
                element,
                parameters: parameters.clone(),
            },
        );
    }

    let cached = buffers.contexts.get_mut(&context)?;
    if cached.parameters != parameters {
        if cached.parameters.bounds != parameters.bounds {
            cached.element.resize(parameters.bounds, None);
        }
        cached.element.update_uniforms(shadow_uniforms(&parameters));
        cached.parameters = parameters;
    }

    Some(cached.element.clone())
}

fn shadow_uniforms(parameters: &ShadowParameters) -> Vec<Uniform<'static>> {
    vec![
        Uniform::new("shadow_rect", parameters.shadow_rect).into_owned(),
        Uniform::new("radius", parameters.radius).into_owned(),
        Uniform::new("blur", parameters.blur).into_owned(),
        Uniform::new("opacity", parameters.opacity).into_owned(),
        Uniform::new("shadow_color", parameters.color).into_owned(),
    ]
}

fn shadow_bounds(
    geometry: Rectangle<i32, Logical>,
    offset_y: f64,
    blur: f64,
) -> Rectangle<i32, Logical> {
    let extent = (blur * 2.0).ceil() as i32;
    let offset_y = offset_y.round() as i32;

    Rectangle::new(
        (geometry.loc.x - extent, geometry.loc.y + offset_y - extent).into(),
        (geometry.size.w + extent * 2, geometry.size.h + extent * 2).into(),
    )
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
    alpha: f32,
    clip_changed: bool,
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
                alpha,
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
            alpha,
            RenderElementKind::Unspecified,
        )
        .into_iter()
        .map(|inner| {
            RoundedSurfaceRenderElement {
                inner,
                program: program.clone(),
                clip_rect: clip,
                radius,
                clip_changed,
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

fn scaled_visual_rect(rect: ferese_layout::Rect, scale: f64) -> ferese_layout::Rect {
    let scale = scale.clamp(0.0, 1.0);
    let width = rect.width * scale;
    let height = rect.height * scale;

    ferese_layout::Rect::new(
        rect.x + (rect.width - width) / 2.0,
        rect.y + (rect.height - height) / 2.0,
        width,
        height,
    )
}

fn color_with_alpha(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] *= alpha;
    color
}

fn rounded_visual_rect(
    rect: ferese_layout::Rect,
    output_location: Point<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let left = (rect.x - f64::from(output_location.x)).round() as i32;
    let top = (rect.y - f64::from(output_location.y)).round() as i32;
    let right = (rect.x + rect.width - f64::from(output_location.x)).round() as i32;
    let bottom = (rect.y + rect.height - f64::from(output_location.y)).round() as i32;

    Rectangle::new(
        (left, top).into(),
        ((right - left).max(1), (bottom - top).max(1)).into(),
    )
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
    state: &mut Ferese,
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
                        .map(|geometry| (geometry, layer.clone()))
                })
            })
            .collect::<Vec<_>>()
    };

    state
        .material_buffers
        .retain(|surface, _| surface.is_alive());

    let mut elements = Vec::new();
    for (geometry, layer) in layers {
        elements.extend(
            AsRenderElements::<GlesRenderer>::render_elements::<
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(
                &layer,
                renderer,
                geometry.loc.to_physical_precise_round(scale),
                scale.into(),
                1.0,
            )
            .into_iter()
            .filter_map(|element| {
                let origin = Point::<i32, Physical>::default();
                let element = RescaleRenderElement::from_element(element, origin, 1.0);
                let element =
                    RelocateRenderElement::from_element(element, origin, Relocate::Relative);

                CropRenderElement::from_element(element, scale, output_crop).map(Into::into)
            }),
        );

        if let Some(material) = material_element(state, renderer, output, &layer, geometry) {
            elements.push(material);
        }
    }

    elements
}

fn material_element(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
    layer: &LayerSurface,
    geometry: Rectangle<i32, Logical>,
) -> Option<AnimatedWindowRenderElement> {
    let surface = layer.wl_surface();
    let (role, generation) = crate::effects::surface_role(surface)?;
    let material = crate::effects::resolve_material(role, state.theme_settings.material_style);
    let mode = output.current_mode()?;
    let scale = output.current_scale().fractional_scale();
    let output_logical = smithay::utils::Size::<i32, Logical>::from((
        (f64::from(mode.size.w) / scale).ceil() as i32,
        (f64::from(mode.size.h) / scale).ceil() as i32,
    ));
    let sample_geometry =
        expanded_blur_region(geometry, material.blur.ceil() as i32, output_logical);
    let sample_physical = sample_geometry.to_physical_precise_round(scale);
    let transform = output.current_transform().invert();
    let tint = [0.067, 0.094, 0.129, material.opacity];
    let parameters = MaterialParameters {
        geometry,
        tint,
        noise: material.noise,
        generation,
        scene_generation: matches!(material.style, crate::config::MaterialStyle::Glass)
            .then(|| state.material_scene_generation())
            .unwrap_or(0),
        opaque: material.opacity == 1.0,
        blur: material.blur * scale as f32,
        saturation: material.saturation,
        brightness: material.brightness,
        sample_geometry,
        sample_physical,
        visible_framebuffer: framebuffer_clip_rect(
            geometry.to_physical_precise_round(scale),
            mode.size,
            transform,
        ),
        sample_framebuffer: framebuffer_clip_rect(sample_physical, mode.size, transform),
    };
    let context = renderer.context_id().erased();
    let needs_replacement = state
        .material_buffers
        .get(surface)
        .and_then(|buffers| buffers.contexts.get(&context))
        .is_none_or(|cached| match (&cached.element, material.style) {
            (CachedMaterialElement::Backdrop(_), crate::config::MaterialStyle::Glass) => {
                cached.parameters.sample_physical.size != parameters.sample_physical.size
            }
            (
                CachedMaterialElement::Tint(_),
                crate::config::MaterialStyle::Translucent | crate::config::MaterialStyle::Solid,
            ) => false,
            _ => true,
        });
    let mut replacement = needs_replacement
        .then(|| match material.style {
            crate::config::MaterialStyle::Glass => {
                let program = backdrop_program(state, renderer)?;
                let texture_size = smithay::utils::Size::<i32, Buffer>::from((
                    sample_physical.size.w,
                    sample_physical.size.h,
                ));
                let texture = Offscreen::<GlesTexture>::create_buffer(
                    renderer,
                    Fourcc::Abgr8888,
                    texture_size,
                )
                .ok()?;
                Some(CachedMaterialElement::Backdrop(BackdropRenderElement::new(
                    texture,
                    program.0,
                    &parameters,
                )))
            }
            crate::config::MaterialStyle::Translucent | crate::config::MaterialStyle::Solid => {
                let program = material_program(state, renderer)?;
                Some(CachedMaterialElement::Tint(PixelShaderElement::new(
                    program.0,
                    geometry,
                    parameters
                        .opaque
                        .then(|| vec![Rectangle::from_size(geometry.size)]),
                    1.0,
                    material_uniforms(&parameters),
                    RenderElementKind::Unspecified,
                )))
            }
        })
        .flatten();
    let buffers = state.material_buffers.entry(surface.clone()).or_default();
    if !buffers.contexts.contains_key(&context) {
        buffers.contexts.insert(
            context.clone(),
            CachedMaterial {
                element: replacement.take()?,
                parameters: parameters.clone(),
            },
        );
    }

    let cached = buffers.contexts.get_mut(&context)?;
    if cached.parameters != parameters {
        match &mut cached.element {
            CachedMaterialElement::Tint(element) => {
                if cached.parameters.geometry != parameters.geometry
                    || cached.parameters.opaque != parameters.opaque
                {
                    let opaque = parameters
                        .opaque
                        .then(|| vec![Rectangle::from_size(parameters.geometry.size)]);
                    element.resize(parameters.geometry, opaque);
                }
                element.update_uniforms(material_uniforms(&parameters));
            }
            CachedMaterialElement::Backdrop(element)
                if cached.parameters.sample_physical.size == parameters.sample_physical.size =>
            {
                element.update(&parameters);
            }
            _ => cached.element = replacement.take()?,
        }
        cached.parameters = parameters;
    }

    Some(match &cached.element {
        CachedMaterialElement::Tint(element) => element.clone().into(),
        CachedMaterialElement::Backdrop(element) => element.clone().into(),
    })
}

fn material_program(state: &mut Ferese, renderer: &mut GlesRenderer) -> Option<MaterialProgram> {
    let context = renderer.context_id().erased();
    if let Some(program) = state.material_programs.get(&context) {
        return Some(program.clone());
    }

    let uniforms = [
        UniformName::new("tint", UniformType::_4f),
        UniformName::new("noise_amount", UniformType::_1f),
    ];
    match renderer.compile_custom_pixel_shader(MATERIAL_SHADER, &uniforms) {
        Ok(program) => {
            let program = MaterialProgram(program);
            state.material_programs.insert(context, program.clone());
            Some(program)
        }
        Err(error) => {
            tracing::error!(%error, "failed to compile semantic material shader");
            None
        }
    }
}

fn backdrop_program(state: &mut Ferese, renderer: &mut GlesRenderer) -> Option<BackdropProgram> {
    let context = renderer.context_id().erased();
    if let Some(program) = state.backdrop_programs.get(&context) {
        return Some(program.clone());
    }

    let uniforms = [
        UniformName::new("visible_rect", UniformType::_4f),
        UniformName::new("texture_size", UniformType::_2f),
        UniformName::new("blur_radius", UniformType::_1f),
        UniformName::new("saturation", UniformType::_1f),
        UniformName::new("brightness", UniformType::_1f),
        UniformName::new("tint", UniformType::_4f),
        UniformName::new("noise_amount", UniformType::_1f),
    ];
    match renderer.compile_custom_texture_shader(BACKDROP_SHADER, &uniforms) {
        Ok(program) => {
            let program = BackdropProgram(program);
            state.backdrop_programs.insert(context, program.clone());
            Some(program)
        }
        Err(error) => {
            tracing::error!(%error, "failed to compile backdrop material shader");
            None
        }
    }
}

fn material_uniforms(parameters: &MaterialParameters) -> Vec<Uniform<'static>> {
    vec![
        Uniform::new("tint", parameters.tint).into_owned(),
        Uniform::new("noise_amount", parameters.noise).into_owned(),
    ]
}

fn backdrop_uniforms(parameters: &MaterialParameters) -> Vec<Uniform<'static>> {
    vec![
        Uniform::new("visible_rect", parameters.visible_framebuffer).into_owned(),
        Uniform::new(
            "texture_size",
            [
                parameters.sample_physical.size.w as f32,
                parameters.sample_physical.size.h as f32,
            ],
        )
        .into_owned(),
        Uniform::new("blur_radius", parameters.blur).into_owned(),
        Uniform::new("saturation", parameters.saturation).into_owned(),
        Uniform::new("brightness", parameters.brightness).into_owned(),
        Uniform::new("tint", parameters.tint).into_owned(),
        Uniform::new("noise_amount", parameters.noise).into_owned(),
    ]
}

fn framebuffer_capture_rect(rect: [f32; 4]) -> [i32; 4] {
    [
        rect[0].round() as i32,
        rect[1].round() as i32,
        rect[2].round() as i32,
        rect[3].round() as i32,
    ]
}

fn expanded_blur_region(
    visible: Rectangle<i32, Logical>,
    radius: i32,
    output_size: smithay::utils::Size<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let radius = radius.max(0);
    let left = (visible.loc.x - radius).max(0);
    let top = (visible.loc.y - radius).max(0);
    let right = (visible.loc.x + visible.size.w + radius).min(output_size.w);
    let bottom = (visible.loc.y + visible.size.h + radius).min(output_size.h);

    Rectangle::new(
        (left, top).into(),
        ((right - left).max(1), (bottom - top).max(1)).into(),
    )
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
    use smithay::{
        backend::renderer::{
            damage::OutputDamageTracker,
            element::{Element, Id},
            utils::CommitCounter,
        },
        utils::{Buffer, Logical, Physical, Rectangle, Scale, Transform},
    };

    use super::{
        color_with_alpha, expanded_blur_region, framebuffer_capture_rect, framebuffer_clip_rect,
        normalized_scale, rounded_visual_rect, scaled_visual_rect, shadow_bounds,
    };

    #[derive(Debug)]
    struct DamageElement {
        id: Id,
        geometry: Rectangle<i32, Logical>,
    }

    impl DamageElement {
        fn new(geometry: Rectangle<i32, Logical>) -> Self {
            Self {
                id: Id::new(),
                geometry,
            }
        }
    }

    impl Element for DamageElement {
        fn id(&self) -> &Id {
            &self.id
        }

        fn current_commit(&self) -> CommitCounter {
            CommitCounter::default()
        }

        fn src(&self) -> Rectangle<f64, Buffer> {
            Rectangle::from_size(
                self.geometry
                    .size
                    .to_f64()
                    .to_buffer(1.0, Transform::Normal),
            )
        }

        fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
            self.geometry.to_physical_precise_round(scale)
        }
    }

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

    #[test]
    fn blur_sampling_expands_around_the_visible_surface() {
        let visible = Rectangle::<i32, Logical>::new((100, 80).into(), (400, 300).into());

        assert_eq!(
            expanded_blur_region(visible, 24, (1_000, 800).into()),
            Rectangle::new((76, 56).into(), (448, 348).into())
        );
    }

    #[test]
    fn blur_sampling_clips_to_output_edges() {
        let visible = Rectangle::<i32, Logical>::new((0, 760).into(), (300, 40).into());

        assert_eq!(
            expanded_blur_region(visible, 24, (1_000, 800).into()),
            Rectangle::new((0, 736).into(), (324, 64).into())
        );
    }

    #[test]
    fn framebuffer_capture_rounds_fractional_bounds() {
        assert_eq!(
            framebuffer_capture_rect([10.4, 20.6, 300.2, 199.8]),
            [10, 21, 300, 200]
        );
    }

    #[test]
    fn shadow_bounds_include_blur_and_offset() {
        let geometry = Rectangle::<i32, Logical>::new((100, 80).into(), (400, 300).into());

        assert_eq!(
            shadow_bounds(geometry, 4.0, 18.0),
            Rectangle::new((64, 48).into(), (472, 372).into())
        );
        assert_eq!(
            shadow_bounds(geometry, -6.0, 10.0),
            Rectangle::new((80, 54).into(), (440, 340).into())
        );
    }

    #[test]
    fn damage_tracks_old_and_new_expanded_shadow_bounds() {
        let old_window = Rectangle::<i32, Logical>::new((100, 80).into(), (400, 300).into());
        let new_window = Rectangle::<i32, Logical>::new((300, 180).into(), (400, 300).into());
        let mut shadow = DamageElement::new(shadow_bounds(old_window, 4.0, 18.0));
        let mut tracker =
            OutputDamageTracker::new((1_000, 800), Scale::from(1.0), Transform::Normal);

        tracker.damage_output(0, &[&shadow]).unwrap();
        shadow.geometry = shadow_bounds(new_window, 4.0, 18.0);
        let damage = tracker
            .damage_output(1, &[&shadow])
            .unwrap()
            .0
            .cloned()
            .unwrap();

        assert!(damage.iter().any(|rect| rect.contains((64, 48))));
        assert!(damage.iter().any(|rect| rect.contains((735, 519))));

        let removal_damage = tracker
            .damage_output::<&DamageElement>(1, &[])
            .unwrap()
            .0
            .cloned()
            .unwrap();
        assert!(removal_damage.iter().any(|rect| rect.contains((300, 180))));
    }

    #[test]
    fn close_transform_scales_about_the_window_center() {
        let rect = ferese_layout::Rect::new(100.0, 50.0, 400.0, 300.0);

        assert_eq!(
            scaled_visual_rect(rect, 0.98),
            ferese_layout::Rect::new(104.0, 53.0, 392.0, 294.0)
        );
        assert_eq!(scaled_visual_rect(rect, 1.0), rect);
    }

    #[test]
    fn close_opacity_only_changes_the_alpha_channel() {
        assert_eq!(
            color_with_alpha([0.2, 0.4, 0.6, 0.8], 0.5),
            [0.2, 0.4, 0.6, 0.4]
        );
    }

    #[test]
    fn visual_rect_rounds_shared_edges_instead_of_size() {
        let rect = ferese_layout::Rect::new(100.5, 50.25, 399.5, 299.75);
        let rounded = rounded_visual_rect(rect, (0, 0).into());

        assert_eq!(rounded, Rectangle::new((101, 50).into(), (399, 300).into()));
        assert_eq!(rounded.loc.x + rounded.size.w, 500);
        assert_eq!(rounded.loc.y + rounded.size.h, 350);

        for tenth in 0..=10 {
            let left = 100.0 + f64::from(tenth) / 10.0;
            let frame = rounded_visual_rect(
                ferese_layout::Rect::new(left, 50.0, 500.0 - left, 300.0),
                (0, 0).into(),
            );

            assert_eq!(frame.loc.x + frame.size.w, 500);
        }
    }
}
