use std::{
    collections::HashMap,
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Color32F, ErasedContextId, Frame, ImportDma, Offscreen, Renderer, Texture,
            damage::OutputDamageTracker,
            element::{
                Element, Id, Kind as RenderElementKind, RenderElement, UnderlyingStorage,
                memory::MemoryRenderBufferRenderElement,
                render_elements,
                solid::{SolidColorBuffer, SolidColorRenderElement},
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
        Buffer, Clock, Logical, Monotonic, Physical, Point, Rectangle, Scale as RenderScale, Size,
        Transform,
    },
    wayland::shell::wlr_layer::Layer,
    wayland::{compositor::with_states, presentation::Refresh},
};

use crate::{
    Ferese,
    metrics::{FrameEffectMetrics, RenderMetrics},
};

type SurfaceRenderElement = CropRenderElement<
    RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
>;

type WindowRenderElement =
    CropRenderElement<RelocateRenderElement<RescaleRenderElement<WindowContentRenderElement>>>;

type MemoryRenderElement = CropRenderElement<
    RelocateRenderElement<RescaleRenderElement<MemoryRenderBufferRenderElement<GlesRenderer>>>,
>;

type PhysicalDamage = Vec<Rectangle<i32, Physical>>;
type DamageRenderResult = Result<(Option<PhysicalDamage>, FrameEffectMetrics), Box<dyn Error>>;

render_elements! {
    pub(crate) AnimatedWindowRenderElement<=GlesRenderer>;
    Window=WindowRenderElement,
    Surface=SurfaceRenderElement,
    Memory=MemoryRenderElement,
    Solid=SolidColorRenderElement,
    Border=PixelShaderElement,
    Blur=BlurRenderElement,
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

precision highp float;
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

// The material program paints either the fallback tint, the edges above client
// content, or the shadow outside the surface. All coordinates are physical px.
const MATERIAL_SHADER: &str = r#"
precision highp float;
uniform float alpha;
uniform vec4 tint;
uniform vec4 visible_rect;
uniform float material_radius;
uniform float paint_mode;
uniform vec4 shadow_rect;
uniform vec2 shadow_values;
varying vec2 v_coords;

float rounded_distance(vec2 point, vec4 rect) {
    vec2 half_size = rect.zw * 0.5;
    float radius = min(material_radius, min(half_size.x, half_size.y));
    vec2 d = abs(point - rect.xy - half_size) - (half_size - vec2(radius));
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - radius;
}
void main() {
    float sdf = rounded_distance(gl_FragCoord.xy, visible_rect);
    float coverage = 1.0 - smoothstep(-0.5, 0.5, sdf);
    if (paint_mode > 0.5) {
        float distance = max(rounded_distance(gl_FragCoord.xy, shadow_rect), 0.0);
        float sigma = max(shadow_values.x * 0.5, 0.5);
        float opacity = exp(-0.5 * distance * distance / (sigma * sigma))
            * shadow_values.y * (1.0 - coverage) * alpha;
        gl_FragColor = vec4(0.0, 0.0, 0.0, opacity);
    } else {
        float opacity = tint.a * coverage * alpha;
        gl_FragColor = vec4(tint.rgb * opacity, opacity);
    }
}
"#;

const BLUR_SHADER: &str = r#"#version 100
//_DEFINES_
#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif
precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform float alpha;
uniform vec4 visible_rect;
uniform float material_radius;
uniform vec2 texture_size;
uniform float blur_radius;
uniform vec4 tint;
varying vec2 v_coords;
void main() {
    vec2 half_size = visible_rect.zw * 0.5;
    vec2 d = abs(gl_FragCoord.xy - visible_rect.xy - half_size) - (half_size - vec2(material_radius));
    float sdf = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - material_radius;
    float coverage = alpha * (1.0 - smoothstep(-0.5, 0.5, sdf));
    if (coverage <= 0.0) { gl_FragColor = vec4(0.0); return; }
    // Denser Gaussian sampling preserves the same radius and softness without
    // the visible grid left by widely spaced taps on detailed wallpapers.
    vec2 step_size = vec2(blur_radius / 6.0) / max(texture_size, vec2(1.0));
    vec4 color = vec4(0.0);
    float weights = 0.0;
    for (int y=-6; y<=6; y++) {
        for (int x=-6; x<=6; x++) {
            float weight = exp(-float(x*x+y*y) / 18.0);
            color += texture2D(tex, v_coords + vec2(float(x),float(y))*step_size) * weight;
            weights += weight;
        }
    }
    vec3 background = color.rgb / weights;
    gl_FragColor = vec4(mix(background, tint.rgb, tint.a) * coverage, coverage);
}
"#;

#[derive(Clone, Debug)]
pub(crate) struct BlurProgram(GlesTexProgram);

#[derive(Clone, Debug)]
struct BlurCapture {
    texture: GlesTexture,
    dirty: Arc<AtomicBool>,
    geometry: Rectangle<i32, Logical>,
}

#[derive(Clone, Debug)]
struct BlurRenderElement {
    capture_dirty: Arc<AtomicBool>,
    texture: GlesTexture,
    program: GlesTexProgram,
    id: Id,
    commit: CommitCounter,
    geometry: Rectangle<i32, Logical>,
    capture_rect: [i32; 4],
    uniforms: Vec<Uniform<'static>>,
}

impl BlurRenderElement {
    fn new(
        texture: GlesTexture,
        program: GlesTexProgram,
        parameters: &MaterialParameters,
        capture_dirty: Arc<AtomicBool>,
    ) -> Self {
        Self {
            capture_dirty,
            texture,
            program,
            id: Id::new(),
            commit: CommitCounter::default(),
            geometry: parameters.sample_geometry,
            capture_rect: framebuffer_capture_rect(parameters.sample_framebuffer),
            uniforms: blur_uniforms(parameters),
        }
    }

    fn update(&mut self, parameters: &MaterialParameters) {
        self.capture_dirty.store(true, Ordering::Relaxed);
        self.geometry = parameters.sample_geometry;
        self.capture_rect = framebuffer_capture_rect(parameters.sample_framebuffer);
        self.uniforms = blur_uniforms(parameters);
        self.commit.increment();
    }
}

impl Element for BlurRenderElement {
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
        blur_damage(self.geometry(scale).size, self.commit, commit)
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

fn blur_damage(
    size: Size<i32, Physical>,
    current: CommitCounter,
    previous: Option<CommitCounter>,
) -> DamageSet<i32, Physical> {
    if previous == Some(current) {
        DamageSet::default()
    } else {
        // A new scene needs its entire sampling halo repainted before capture.
        DamageSet::from_slice(&[Rectangle::from_size(size)])
    }
}

impl RenderElement<GlesRenderer> for BlurRenderElement {
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
        // Buffer-age restoration and unrelated partial damage must reuse the
        // clean captured scene. Recapturing a partly repainted framebuffer
        // would feed previous blurred pixels back into the blur.
        if self.capture_dirty.load(Ordering::Relaxed) {
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
            self.capture_dirty.store(false, Ordering::Relaxed);
        }

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

#[derive(Clone, Debug)]
pub(crate) struct RoundedClipPrograms {
    texture: GlesTexProgram,
    border: GlesPixelProgram,
    shadow: GlesPixelProgram,
}

#[derive(Debug)]
pub(crate) struct OverviewScrim {
    buffer: SolidColorBuffer,
}

impl OverviewScrim {
    fn new(size: Size<i32, Logical>) -> Self {
        Self {
            buffer: SolidColorBuffer::new(size, [0.02, 0.025, 0.035, 0.62]),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MaterialProgram(GlesPixelProgram);

#[derive(Clone, Debug, PartialEq)]
struct MaterialParameters {
    blur: f32,
    scene_generation: u64,
    sample_geometry: Rectangle<i32, Logical>,
    sample_physical: Rectangle<i32, Physical>,
    sample_framebuffer: [f32; 4],
    radius: f32,
    shadow_rect: [f32; 4],
    shadow_values: [f32; 2],
    shadow_bounds: Rectangle<i32, Logical>,
    geometry: Rectangle<i32, Logical>,
    tint: [f32; 4],
    generation: u64,
    opaque: bool,
    visible_framebuffer: [f32; 4],
}

#[derive(Clone, Debug)]
enum MaterialElement {
    Fill(PixelShaderElement),
    Blur(BlurRenderElement),
}

#[derive(Debug)]
struct CachedMaterial {
    element: MaterialElement,
    parameters: MaterialParameters,
    shadow: PixelShaderElement,
}

#[derive(Debug, Default)]
pub(crate) struct MaterialBuffers {
    contexts: HashMap<(ErasedContextId, usize), CachedMaterial>,
    captures: HashMap<ErasedContextId, BlurCapture>,
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
    blur: f32,
    bounds: Rectangle<i32, Logical>,
    shadow_rect: [f32; 4],
    radius: f32,
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
                        state.process_dmabuf_imports(renderer);
                        let elements = animated_window_elements(state, renderer, &output);
                        let effects = frame_effect_metrics(
                            &elements,
                            output.current_scale().fractional_scale(),
                        );
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

                        Ok((result.damage.cloned(), effects))
                    }
                })();
                let (damage, effects) = match rendered {
                    Ok((Some(damage), effects)) => (damage, effects),
                    Ok((None, _)) => {
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
                render_metrics.record_frame(render_started.elapsed(), &damage, 0, effects);

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

pub(crate) fn frame_effect_metrics(
    _elements: &[AnimatedWindowRenderElement],
    _scale: f64,
) -> FrameEffectMetrics {
    FrameEffectMetrics::default()
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
    let upper_layers: &[Layer] = if state.output_has_fullscreen(output) {
        &[Layer::Overlay]
    } else {
        &[Layer::Overlay, Layer::Top]
    };
    elements.extend(layer_elements(state, renderer, output, upper_layers));
    let windows = state
        .space
        .elements()
        .rev()
        .filter_map(|window| {
            let id = *state.window_ids.get(window)?;
            let visual = state.presented_window_rect(id)?;
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

        let dim = state.window_dimming.get(&id).map_or(0.0, |dim| dim.current);
        if let Some(overlay) = window_dim_element(
            state,
            renderer,
            id,
            constrain,
            scale,
            state.theme_settings.window_radius * decoration_progress,
            dim as f32 * close_alpha,
            output,
        ) {
            // Front-to-back: dim the application and its border, not its shadow
            // or other windows/layer surfaces. The overlay is input-transparent.
            elements.push(overlay.into());
        }

        if let Some(programs) = rounded_clip_program.clone() {
            let window_radius = state.theme_settings.window_radius * decoration_progress;
            let shadow_offset_y = state.theme_settings.shadow_offset_y;
            let shadow_blur = state.theme_settings.shadow_blur;
            let shadow_opacity = state.theme_settings.shadow_opacity;
            let shadow_color = state.theme_settings.shadow_color.0;
            let focused = if state.overview.is_active() {
                state.overview_selected(id)
            } else {
                state.focused_window == Some(id)
            };
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
    if state.overview.is_presenting()
        && let Some(scrim) = overview_scrim_element(state, output, output_geometry, scale)
    {
        elements.push(scrim.into());
    }
    elements.extend(layer_elements(
        state,
        renderer,
        output,
        &[Layer::Bottom, Layer::Background],
    ));
    elements
}

fn overview_scrim_element(
    state: &mut Ferese,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    scale: f64,
) -> Option<SolidColorRenderElement> {
    let output_id = state.output_id(output)?;
    let scrim = state
        .overview_scrims
        .entry(output_id)
        .or_insert_with(|| OverviewScrim::new(output_geometry.size));

    scrim
        .buffer
        .update(output_geometry.size, [0.02, 0.025, 0.035, 0.62]);

    Some(SolidColorRenderElement::from_buffer(
        &scrim.buffer,
        Point::<i32, Physical>::default(),
        scale,
        1.0,
        RenderElementKind::Unspecified,
    ))
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
fn window_dim_element(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    id: ferese_layout::WindowId,
    geometry: Rectangle<i32, Logical>,
    scale: f64,
    radius: f64,
    amount: f32,
    output: &Output,
) -> Option<PixelShaderElement> {
    if amount <= 0.0 {
        state.window_dims.remove(&id);
        return None;
    }
    let mode = output.current_mode()?;
    let program = material_program(state, renderer)?;
    let parameters = BorderParameters {
        geometry,
        clip_rect: framebuffer_clip_rect(
            geometry.to_physical_precise_round(scale),
            mode.size,
            output.current_transform().invert(),
        ),
        radius: scaled_effect_value(radius, geometry, scale),
        width: 0.0,
        color: [0.0, 0.0, 0.0, amount.clamp(0.0, 1.0)],
    };
    let uniforms = |p: &BorderParameters| {
        vec![
            Uniform::new("visible_rect", p.clip_rect).into_owned(),
            Uniform::new("material_radius", p.radius).into_owned(),
            Uniform::new("tint", p.color).into_owned(),
            Uniform::new("paint_mode", 0.0_f32).into_owned(),
            Uniform::new("shadow_rect", p.clip_rect).into_owned(),
            Uniform::new("shadow_values", [0.0_f32; 2]).into_owned(),
        ]
    };
    let context = renderer.context_id().erased();
    let buffers = state.window_dims.entry(id).or_default();
    let cached = buffers
        .contexts
        .entry(context)
        .or_insert_with(|| CachedBorder {
            element: PixelShaderElement::new(
                program.0,
                geometry,
                None,
                1.0,
                uniforms(&parameters),
                RenderElementKind::Unspecified,
            ),
            parameters: parameters.clone(),
        });
    if cached.parameters != parameters {
        if cached.parameters.geometry != geometry {
            cached.element.resize(geometry, None);
        }
        // Updating uniforms advances the element's commit so unchanged client
        // buffers still repaint while focus dimming animates.
        cached.element.update_uniforms(uniforms(&parameters));
        cached.parameters = parameters;
    }
    Some(cached.element.clone())
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
        blur: (blur * scale) as f32,
        bounds,
        shadow_rect: framebuffer_clip_rect(
            shadow_geometry.to_physical_precise_round(scale),
            mode.size,
            output.current_transform().invert(),
        ),
        radius: scaled_effect_value(requested_radius, geometry, scale),
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
    // Match GlesFrame's projection: Smithay already accounts for GL's Y axis.
    // An extra bottom-left conversion here mirrors shader clips independently
    // of the surface geometry (notably with the nested Flipped180 output).
    let element_size = transform.transform_size(output_size);
    let transformed = transform.transform_rect_in(geometry, &element_size);

    [
        transformed.loc.x as f32,
        transformed.loc.y as f32,
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
        for (popup, offset) in PopupManager::popups_for_surface(layer.wl_surface()) {
            let mut bounds = popup.geometry();
            let content_origin = geometry.loc + offset - bounds.loc;
            bounds.loc = geometry.loc + offset;
            append_material_surface(
                state,
                renderer,
                output,
                popup.wl_surface(),
                bounds,
                content_origin,
                scale,
                output_crop,
                &mut elements,
            );
        }
        append_material_surface(
            state,
            renderer,
            output,
            layer.wl_surface(),
            geometry,
            geometry.loc,
            scale,
            output_crop,
            &mut elements,
        );
    }

    elements
}

#[allow(clippy::too_many_arguments)]
fn append_material_surface(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    geometry: Rectangle<i32, Logical>,
    content_origin: Point<i32, Logical>,
    scale: f64,
    output_crop: Rectangle<i32, Physical>,
    elements: &mut Vec<AnimatedWindowRenderElement>,
) {
    let radius = if crate::effects::surface_role(surface)
        .is_some_and(|(role, _)| role == crate::effects::SemanticRole::Panel)
    {
        state.theme_settings.panel_radius
    } else {
        state.theme_settings.material_radius
    };
    let regions = crate::effects::surface_regions(surface);
    let targets: Vec<_> = match &regions {
        None => vec![(geometry, radius as f32)],
        Some(regions) => regions
            .iter()
            .filter_map(|r| {
                Rectangle::new(
                    (content_origin.x + r[0], content_origin.y + r[1]).into(),
                    (r[2], r[3]).into(),
                )
                .intersection(geometry)
                .map(|rect| (rect, r[4] as f32))
            })
            .collect(),
    };
    let materials: Vec<_> = targets
        .iter()
        .enumerate()
        .filter_map(|(index, (rect, radius))| {
            material_element(
                state, renderer, output, surface, *rect, *radius, index, geometry,
            )
        })
        .collect();
    if let Some(buffers) = state.material_buffers.get_mut(surface) {
        buffers
            .contexts
            .retain(|(_, index), _| *index < targets.len());
    }
    let material = (!materials.is_empty() && regions.is_none()).then_some(());
    let clip = material.as_ref().and_then(|_| {
        let mode = output.current_mode()?;
        let program = rounded_clip_program(state, renderer)?;
        Some((
            program.texture,
            framebuffer_clip_rect(
                geometry.to_physical_precise_round(scale),
                mode.size,
                output.current_transform().invert(),
            ),
            scaled_effect_value(radius, geometry, scale),
        ))
    });
    // Smithay element lists are front to back: content, material, shadow.
    let content = render_elements_from_surface_tree::<
        GlesRenderer,
        WaylandSurfaceRenderElement<GlesRenderer>,
    >(
        renderer,
        surface,
        content_origin.to_physical_precise_round(scale),
        scale,
        1.0,
        RenderElementKind::Unspecified,
    );
    elements.extend(content.into_iter().filter_map(|element| {
        let origin = Point::<i32, Physical>::default();
        let element = if let Some((program, clip_rect, radius)) = &clip {
            WindowContentRenderElement::Rounded(RoundedSurfaceRenderElement {
                inner: element,
                program: program.clone(),
                clip_rect: *clip_rect,
                radius: *radius,
                clip_changed: false,
            })
        } else {
            WindowContentRenderElement::Popup(element)
        };
        let element = RescaleRenderElement::from_element(element, origin, 1.0);
        let element = RelocateRenderElement::from_element(element, origin, Relocate::Relative);
        CropRenderElement::from_element(element, scale, output_crop).map(Into::into)
    }));
    for (background, shadow) in materials {
        elements.push(background);
        elements.push(shadow);
    }
}

fn material_element(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    geometry: Rectangle<i32, Logical>,
    radius: f32,
    index: usize,
    capture_geometry: Rectangle<i32, Logical>,
) -> Option<(AnimatedWindowRenderElement, AnimatedWindowRenderElement)> {
    let (role, generation) = crate::effects::surface_role(surface)?;
    let mut material = crate::effects::resolve_material(role, state.theme_settings.material_style);
    if role == crate::effects::SemanticRole::Panel
        && material.style == crate::config::MaterialStyle::Translucent
    {
        material.opacity = state.theme_settings.panel_opacity as f32;
    }
    let mode = output.current_mode()?;
    let scale = output.current_scale().fractional_scale();
    let transform = output.current_transform().invert();
    let [red, green, blue, _] = state.theme_settings.surface_base_color.0;
    let radius = radius.min(geometry.size.w.min(geometry.size.h) as f32 * 0.5);
    let [offset_y, shadow_blur, shadow_opacity] = material.shadow;
    let edge_bar =
        role == crate::effects::SemanticRole::Panel && radius == 0.0 && geometry.loc.y == 0;
    let shadow_opacity = if edge_bar { 0.0 } else { shadow_opacity };
    let shadow_geometry = Rectangle::new(
        (geometry.loc.x, geometry.loc.y + offset_y.round() as i32).into(),
        geometry.size,
    );
    let blur = if material.style == crate::config::MaterialStyle::Translucent {
        state.theme_settings.backdrop_blur
    } else {
        0.0
    };
    let output_size = output
        .current_transform()
        .transform_size(mode.size)
        .to_f64()
        .to_logical(scale)
        .to_i32_ceil();
    let sample_geometry = expanded_blur_region(capture_geometry, blur.ceil() as i32, output_size);
    let sample_physical = sample_geometry.to_physical_precise_round(scale);
    let parameters = MaterialParameters {
        blur: (blur * scale) as f32,
        scene_generation: if blur > 0.0 {
            state.backdrop_generation
        } else {
            0
        },
        sample_geometry,
        sample_physical,
        sample_framebuffer: framebuffer_clip_rect(sample_physical, mode.size, transform),
        radius: radius * scale as f32,
        shadow_rect: framebuffer_clip_rect(
            shadow_geometry.to_physical_precise_round(scale),
            mode.size,
            transform,
        ),
        shadow_values: [(shadow_blur * scale) as f32, shadow_opacity as f32],
        shadow_bounds: shadow_bounds(geometry, offset_y, shadow_blur),
        geometry,
        tint: [red, green, blue, material.opacity],
        generation,
        opaque: material.opacity == 1.0 && radius == 0.0,
        visible_framebuffer: framebuffer_clip_rect(
            geometry.to_physical_precise_round(scale),
            mode.size,
            transform,
        ),
    };
    let context = renderer.context_id().erased();
    let program = material_program(state, renderer)?;
    let blur_program = (blur > 0.0)
        .then(|| blur_program(state, renderer))
        .flatten();
    let buffers = state.material_buffers.entry(surface.clone()).or_default();
    let capture = if let Some(blur_program) = blur_program {
        if buffers.captures.get(&context).is_none_or(|c| {
            c.geometry != sample_geometry
                || c.texture.size() != Size::from((sample_physical.size.w, sample_physical.size.h))
        }) {
            let texture = Offscreen::<GlesTexture>::create_buffer(
                renderer,
                Fourcc::Abgr8888,
                (sample_physical.size.w, sample_physical.size.h).into(),
            )
            .ok();
            if let Some(texture) = texture {
                buffers.captures.insert(
                    context.clone(),
                    BlurCapture {
                        texture,
                        dirty: Arc::new(AtomicBool::new(true)),
                        geometry: sample_geometry,
                    },
                );
            } else {
                buffers.captures.remove(&context);
            }
        }
        buffers
            .captures
            .get(&context)
            .cloned()
            .map(|capture| (capture, blur_program))
    } else {
        buffers.captures.remove(&context);
        None
    };
    let make_element = || {
        if let Some((capture, program)) = &capture {
            MaterialElement::Blur(BlurRenderElement::new(
                capture.texture.clone(),
                program.0.clone(),
                &parameters,
                capture.dirty.clone(),
            ))
        } else {
            MaterialElement::Fill(PixelShaderElement::new(
                program.0.clone(),
                geometry,
                parameters
                    .opaque
                    .then(|| vec![Rectangle::from_size(geometry.size)]),
                1.0,
                material_uniforms(&parameters),
                RenderElementKind::Unspecified,
            ))
        }
    };
    let cached = buffers
        .contexts
        .entry((context, index))
        .or_insert_with(|| CachedMaterial {
            element: make_element(),
            parameters: parameters.clone(),
            shadow: PixelShaderElement::new(
                program.0.clone(),
                parameters.shadow_bounds,
                None,
                1.0,
                decoration_uniforms(&parameters, 2.0),
                RenderElementKind::Unspecified,
            ),
        });
    let replace = match &cached.element {
        MaterialElement::Fill(_) => capture.is_some(),
        MaterialElement::Blur(element) => capture
            .as_ref()
            .is_none_or(|(capture, _)| capture.texture.tex_id() != element.texture.tex_id()),
    };
    if replace {
        cached.element = make_element();
    }
    if cached.parameters != parameters {
        cached.shadow.resize(parameters.shadow_bounds, None);
        cached
            .shadow
            .update_uniforms(decoration_uniforms(&parameters, 2.0));
        match &mut cached.element {
            MaterialElement::Fill(element) => {
                element.resize(
                    geometry,
                    parameters
                        .opaque
                        .then(|| vec![Rectangle::from_size(geometry.size)]),
                );
                element.update_uniforms(material_uniforms(&parameters));
            }
            MaterialElement::Blur(element) => element.update(&parameters),
        }
        cached.parameters = parameters;
    }
    let background = match &cached.element {
        MaterialElement::Fill(element) => element.clone().into(),
        MaterialElement::Blur(element) => element.clone().into(),
    };
    Some((background, cached.shadow.clone().into()))
}

fn blur_program(state: &mut Ferese, renderer: &mut GlesRenderer) -> Option<BlurProgram> {
    let context = renderer.context_id().erased();
    if let Some(program) = state.blur_programs.get(&context) {
        return Some(program.clone());
    }
    let uniforms = [
        UniformName::new("visible_rect", UniformType::_4f),
        UniformName::new("material_radius", UniformType::_1f),
        UniformName::new("texture_size", UniformType::_2f),
        UniformName::new("blur_radius", UniformType::_1f),
        UniformName::new("tint", UniformType::_4f),
    ];
    match renderer.compile_custom_texture_shader(BLUR_SHADER, &uniforms) {
        Ok(program) => {
            let program = BlurProgram(program);
            state.blur_programs.insert(context, program.clone());
            Some(program)
        }
        Err(error) => {
            tracing::warn!(%error,"backdrop blur unavailable; using plain translucent fill");
            None
        }
    }
}

fn blur_uniforms(p: &MaterialParameters) -> Vec<Uniform<'static>> {
    vec![
        Uniform::new("visible_rect", p.visible_framebuffer).into_owned(),
        Uniform::new("material_radius", p.radius).into_owned(),
        Uniform::new(
            "texture_size",
            [
                p.sample_physical.size.w as f32,
                p.sample_physical.size.h as f32,
            ],
        )
        .into_owned(),
        Uniform::new("blur_radius", p.blur).into_owned(),
        Uniform::new("tint", p.tint).into_owned(),
    ]
}

fn framebuffer_capture_rect(rect: [f32; 4]) -> [i32; 4] {
    rect.map(|v| v.round() as i32)
}
fn expanded_blur_region(
    visible: Rectangle<i32, Logical>,
    radius: i32,
    output_size: Size<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let left = (visible.loc.x - radius).max(0);
    let top = (visible.loc.y - radius).max(0);
    let right = (visible.loc.x + visible.size.w + radius).min(output_size.w);
    let bottom = (visible.loc.y + visible.size.h + radius).min(output_size.h);
    Rectangle::new(
        (left, top).into(),
        ((right - left).max(1), (bottom - top).max(1)).into(),
    )
}

fn material_program(state: &mut Ferese, renderer: &mut GlesRenderer) -> Option<MaterialProgram> {
    let context = renderer.context_id().erased();
    if let Some(program) = state.material_programs.get(&context) {
        return Some(program.clone());
    }

    let uniforms = [
        UniformName::new("visible_rect", UniformType::_4f),
        UniformName::new("material_radius", UniformType::_1f),
        UniformName::new("tint", UniformType::_4f),
        UniformName::new("paint_mode", UniformType::_1f),
        UniformName::new("shadow_rect", UniformType::_4f),
        UniformName::new("shadow_values", UniformType::_2f),
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

fn material_uniforms(parameters: &MaterialParameters) -> Vec<Uniform<'static>> {
    decoration_uniforms(parameters, 0.0)
}

fn decoration_uniforms(parameters: &MaterialParameters, paint_mode: f32) -> Vec<Uniform<'static>> {
    vec![
        Uniform::new("paint_mode", paint_mode).into_owned(),
        Uniform::new("shadow_rect", parameters.shadow_rect).into_owned(),
        Uniform::new("shadow_values", parameters.shadow_values).into_owned(),
        Uniform::new("visible_rect", parameters.visible_framebuffer).into_owned(),
        Uniform::new("material_radius", parameters.radius).into_owned(),
        Uniform::new("tint", parameters.tint).into_owned(),
    ]
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
        color_with_alpha, framebuffer_clip_rect, normalized_scale, rounded_visual_rect,
        scaled_visual_rect, shadow_bounds,
    };

    #[derive(Debug)]
    struct DamageElement {
        id: Id,
        geometry: Rectangle<i32, Logical>,
        commit: CommitCounter,
    }

    impl DamageElement {
        fn new(geometry: Rectangle<i32, Logical>) -> Self {
            Self {
                id: Id::new(),
                geometry,
                commit: CommitCounter::default(),
            }
        }
    }

    impl Element for DamageElement {
        fn id(&self) -> &Id {
            &self.id
        }

        fn current_commit(&self) -> CommitCounter {
            self.commit
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

        fn damage_since(
            &self,
            scale: Scale<f64>,
            commit: Option<CommitCounter>,
        ) -> smithay::backend::renderer::utils::DamageSet<i32, Physical> {
            if commit == Some(self.current_commit()) {
                smithay::backend::renderer::utils::DamageSet::default()
            } else {
                smithay::backend::renderer::utils::DamageSet::from_slice(&[Rectangle::from_size(
                    self.geometry(scale).size,
                )])
            }
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
    fn clip_rect_matches_smithay_gles_projection() {
        let geometry = Rectangle::<i32, Physical>::new((10, 5).into(), (30, 40).into());

        assert_eq!(
            framebuffer_clip_rect(geometry, (100, 80).into(), Transform::Normal),
            [10.0, 5.0, 30.0, 40.0]
        );
    }

    #[test]
    fn clip_rect_follows_output_transform() {
        let geometry = Rectangle::<i32, Physical>::new((10, 5).into(), (30, 40).into());

        assert_eq!(
            framebuffer_clip_rect(geometry, (100, 80).into(), Transform::Flipped180),
            [10.0, 35.0, 30.0, 40.0]
        );
        assert_eq!(
            framebuffer_clip_rect(geometry, (100, 80).into(), Transform::_180),
            [60.0, 35.0, 30.0, 40.0]
        );
    }

    #[test]
    fn panel_clearance_clip_matches_gles_projection_at_fractional_scales() {
        for scale in [1.0, 1.25, 1.75, 2.0] {
            for transform in [
                Transform::Normal,
                Transform::_90,
                Transform::_180,
                Transform::_270,
                Transform::Flipped,
                Transform::Flipped90,
                Transform::Flipped180,
                Transform::Flipped270,
            ] {
                let output_size = smithay::utils::Size::<i32, Physical>::from((
                    (1280.0 * scale) as i32,
                    (800.0 * scale) as i32,
                ));
                let element_size = transform.transform_size(output_size);
                let logical_height = f64::from(element_size.h) / scale;
                // Bar and exclusion end at 40; the 10px outer gap ends at 50.
                let geometry = Rectangle::<f64, Logical>::new(
                    (10.0, 50.0).into(),
                    (400.0, logical_height - 60.0).into(),
                )
                .to_physical_precise_round(scale);
                let clip = framebuffer_clip_rect(geometry, output_size, transform);
                // Independently reproduce GlesFrame's orthographic projection,
                // transform matrix, GL flip, and viewport mapping for each corner.
                let matrix = transform.matrix();
                let mut xs = Vec::new();
                let mut ys = Vec::new();
                for x in [geometry.loc.x, geometry.loc.x + geometry.size.w] {
                    for y in [geometry.loc.y, geometry.loc.y + geometry.size.h] {
                        let nx = 2.0 * x as f32 / element_size.w as f32 - 1.0;
                        let ny = 1.0 - 2.0 * y as f32 / element_size.h as f32;
                        let tx = matrix[0][0] * nx + matrix[1][0] * ny;
                        let ty = matrix[0][1] * nx + matrix[1][1] * ny;
                        xs.push((tx + 1.0) * output_size.w as f32 / 2.0);
                        ys.push((1.0 - ty) * output_size.h as f32 / 2.0);
                    }
                }
                let min_x = xs.iter().copied().fold(f32::INFINITY, f32::min);
                let max_x = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let min_y = ys.iter().copied().fold(f32::INFINITY, f32::min);
                let max_y = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                for (actual, expected) in
                    clip.into_iter()
                        .zip([min_x, min_y, max_x - min_x, max_y - min_y])
                {
                    assert!(
                        (actual - expected).abs() < 0.001,
                        "{transform:?}, scale={scale}: {actual} != {expected}"
                    );
                }
            }
        }
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
