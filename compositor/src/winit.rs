use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    error::Error,
    rc::Rc,
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
            Bind, Color32F, ErasedContextId, ExportMem, Frame, ImportDma, Offscreen, Renderer,
            Texture,
            damage::OutputDamageTracker,
            element::{
                Element, Id, Kind as RenderElementKind, RenderElement, UnderlyingStorage,
                memory::MemoryRenderBufferRenderElement,
                render_elements,
                solid::SolidColorRenderElement,
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
    reexports::calloop::{
        EventLoop,
        timer::{TimeoutAction, Timer},
    },
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
    presentation::{NativeTextureElement, PhysicalShaderElement, physical_rect},
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
    LockSurface=WaylandSurfaceRenderElement<GlesRenderer>,
    Border=PixelShaderElement,
    Blur=BlurRenderElement,
    Effect=PhysicalShaderElement,
    Native=NativeTextureElement,
}

pub(crate) type NestedBackend = Rc<RefCell<winit::WinitGraphicsBackend<GlesRenderer>>>;

#[derive(Debug)]
pub(crate) struct ResizeSnapshot {
    pub texture: GlesTexture,
    pub context: ErasedContextId,
    pub id: Id,
    pub commit: CommitCounter,
    pub elapsed: Duration,
    pub last_tick: Option<Duration>,
    pub scale: f64,
}

impl ResizeSnapshot {
    pub(crate) fn bytes(&self) -> usize {
        self.texture.size().w as usize * self.texture.size().h as usize * 4
    }
}

pub(crate) fn capture_resize_snapshot(
    renderer: &mut GlesRenderer,
    window: &smithay::desktop::Window,
    geometry: Rectangle<i32, Logical>,
    scale: f64,
    remaining: usize,
) -> Result<Option<ResizeSnapshot>, GlesError> {
    let Some(toplevel) = window.toplevel() else {
        return Ok(None);
    };
    let size: Size<i32, Physical> = geometry.size.to_physical_precise_round(scale);
    let bytes = (size.w.max(0) as usize)
        .saturating_mul(size.h.max(0) as usize)
        .saturating_mul(4);
    if bytes == 0 || bytes > remaining {
        return Ok(None);
    }
    // Called before on_commit_buffer_handler: renderer surface state still
    // owns the previous buffer. Copy only during an actual resize handoff.
    let content = render_elements_from_surface_tree::<
        GlesRenderer,
        WaylandSurfaceRenderElement<GlesRenderer>,
    >(
        renderer,
        toplevel.wl_surface(),
        Point::<i32, Logical>::from((-geometry.loc.x, -geometry.loc.y))
            .to_physical_precise_round(scale),
        scale,
        1.0,
        RenderElementKind::Unspecified,
    );
    if content.is_empty() {
        return Ok(None);
    }
    let mut texture = Offscreen::<GlesTexture>::create_buffer(
        renderer,
        Fourcc::Abgr8888,
        (size.w, size.h).into(),
    )?;
    {
        let mut target = renderer.bind(&mut texture)?;
        let mut frame = renderer.render(&mut target, size, Transform::Normal)?;
        let damage = Rectangle::from_size(size);
        frame.clear(Color32F::new(0.0, 0.0, 0.0, 0.0), &[damage])?;
        draw_render_elements(&mut frame, scale, &content, &[damage])?;
        let _ = frame.finish()?;
    }
    Ok(Some(ResizeSnapshot {
        texture,
        context: renderer.context_id().erased(),
        id: Id::new(),
        commit: CommitCounter::default(),
        elapsed: Duration::ZERO,
        last_tick: None,
        scale,
    }))
}

pub(crate) fn capture_window_buffer(
    renderer: &mut GlesRenderer,
    window: &smithay::desktop::Window,
    geometry: Rectangle<i32, Logical>,
    scale: f64,
) -> Result<crate::handlers::screenshot::CaptureBuffer, String> {
    capture_window_frame(renderer, window, geometry, scale, None)
}

pub(crate) fn capture_window_frame(
    renderer: &mut GlesRenderer,
    window: &smithay::desktop::Window,
    geometry: Rectangle<i32, Logical>,
    scale: f64,
    cursor: Option<(&Ferese, Rectangle<i32, Logical>)>,
) -> Result<crate::handlers::screenshot::CaptureBuffer, String> {
    let mut snapshot =
        capture_resize_snapshot(renderer, window, geometry, scale, 128 * 1024 * 1024)
            .map_err(|error| error.to_string())?
            .ok_or("Window has no capturable content")?;
    let size = snapshot.texture.size();
    if let Some((state, cursor_geometry)) = cursor {
        let elements = cursor_elements(state, renderer, cursor_geometry, scale);
        let mut target = renderer
            .bind(&mut snapshot.texture)
            .map_err(|e| e.to_string())?;
        let mut frame = renderer
            .render(&mut target, (size.w, size.h).into(), Transform::Normal)
            .map_err(|e| e.to_string())?;
        draw_render_elements(
            &mut frame,
            scale,
            &elements,
            &[Rectangle::from_size((size.w, size.h).into())],
        )
        .map_err(|e| e.to_string())?;
        let _ = frame.finish().map_err(|e| e.to_string())?;
    }
    let mapping = renderer
        .copy_texture(
            &snapshot.texture,
            Rectangle::from_size(size),
            Fourcc::Abgr8888,
        )
        .map_err(|error| error.to_string())?;
    if mapping.format() != Some(Fourcc::Abgr8888) {
        return Err("Unsupported window readback format".into());
    }
    let bytes = (size.w as usize)
        .checked_mul(size.h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("Window is too large")?;
    let source = renderer
        .map_texture(&mapping)
        .map_err(|error| error.to_string())?;
    if source.len() < bytes {
        return Err("Incomplete window readback".into());
    }
    let mut pixels = source[..bytes].to_vec();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        let alpha = pixel[3] as u16;
        for channel in &mut pixel[..3] {
            *channel = if alpha == 0 {
                0
            } else {
                ((*channel as u16 * 255 + alpha / 2) / alpha).min(255) as u8
            };
        }
    }
    Ok(crate::handlers::screenshot::CaptureBuffer {
        width: size.w,
        height: size.h,
        stride: size.w as usize * 4,
        pixels,
    })
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
precision highp float;

uniform float alpha;
uniform vec4 clip_rect;
uniform float radius;
uniform float border_width;
uniform vec4 border_color;
uniform vec4 border_color_to;
uniform vec4 gradient_line;
uniform vec4 focus_color;
uniform vec4 focus_color_to;
uniform vec4 focus_gradient_line;
uniform float focus_mix;
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
    float progress = clamp(dot(gl_FragCoord.xy - gradient_line.xy, gradient_line.zw), 0.0, 1.0);
    // Interpolate premultiplied endpoints: a transparent endpoint must not
    // leak its RGB into the visible border or create a dark halo.
    vec4 from = vec4(border_color.rgb * border_color.a, border_color.a);
    vec4 to = vec4(border_color_to.rgb * border_color_to.a, border_color_to.a);
    float focus_progress = clamp(dot(gl_FragCoord.xy - focus_gradient_line.xy, focus_gradient_line.zw), 0.0, 1.0);
    vec4 focus_from = vec4(focus_color.rgb * focus_color.a, focus_color.a);
    vec4 focus_to = vec4(focus_color_to.rgb * focus_color_to.a, focus_color_to.a);
    vec4 color = mix(mix(from, to, progress), mix(focus_from, focus_to, focus_progress), focus_mix) * coverage * alpha;

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
uniform vec2 capture_origin;
uniform float blur_radius;
uniform float presentation_alpha;
uniform float background_opacity;
uniform vec4 tint;
varying vec2 v_coords;
void main() {
    vec2 half_size = visible_rect.zw * 0.5;
    vec2 d = abs(gl_FragCoord.xy - visible_rect.xy - half_size) - (half_size - vec2(material_radius));
    float sdf = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - material_radius;
    float coverage = alpha * presentation_alpha * (1.0 - smoothstep(-0.5, 0.5, sdf));
    if (coverage <= 0.0) { gl_FragColor = vec4(0.0); return; }
    vec2 coords = (gl_FragCoord.xy - capture_origin) / texture_size;
    vec4 color = vec4(0.0);
    float weights = 0.0;
    // A spiral avoids aligning the sampling lattice with wallpaper patterns.
    for (int i = 0; i < 256; i++) {
        float radius = sqrt((float(i) + 0.5) / 256.0);
        float angle = float(i) * 2.39996323;
        vec2 offset = vec2(cos(angle), sin(angle)) * radius * blur_radius / texture_size;
        float weight = exp(-4.5 * radius * radius);
        color += texture2D(tex, coords + offset) * weight;
        weights += weight;
    }
    vec3 background = color.rgb / weights;
    gl_FragColor = vec4(mix(background, tint.rgb, tint.a * background_opacity) * coverage, coverage);
}
"#;

const BLURRED_MATERIAL_TINT: f32 = 0.5;

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
        DamageSet::from_slice(&[Rectangle::from_size((size.w, size.h).into())])
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
            let error = frame.with_context(|gl| unsafe {
                gl.GetError();
                let mut framebuffer = 0;
                let mut previous_read_buffer = 0;
                gl.GetIntegerv(
                    smithay::backend::renderer::gles::ffi::READ_FRAMEBUFFER_BINDING,
                    &mut framebuffer,
                );
                gl.GetIntegerv(
                    smithay::backend::renderer::gles::ffi::READ_BUFFER,
                    &mut previous_read_buffer,
                );
                gl.ReadBuffer(if framebuffer == 0 {
                    smithay::backend::renderer::gles::ffi::BACK
                } else {
                    smithay::backend::renderer::gles::ffi::COLOR_ATTACHMENT0
                });
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
                let error = gl.GetError();
                gl.ReadBuffer(previous_read_buffer as u32);
                error
            })?;
            if error != smithay::backend::renderer::gles::ffi::NO_ERROR {
                tracing::warn!(error, "backdrop framebuffer capture failed");
            }
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum OverviewChromePart {
    Card,
    Outline,
    Strip,
    Caption,
}

#[derive(Debug)]
pub(crate) struct OverviewScrim {
    chrome: HashMap<(u64, OverviewChromePart), HashMap<ErasedContextId, CachedBorder>>,
}

#[derive(Clone, Debug)]
pub(crate) struct MaterialProgram(GlesPixelProgram);

#[derive(Clone, Debug, PartialEq)]
struct MaterialParameters {
    presentation_alpha: f32,
    background_opacity: f32,
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
    color_to: [f32; 4],
    gradient_line: [f32; 4],
    focus_color: [f32; 4],
    focus_color_to: [f32; 4],
    focus_gradient_line: [f32; 4],
    focus_mix: f32,
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
    let mut output_scale = initial_scale;
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
                        Some(Mode {
                            size,
                            refresh: rate,
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
                            render_metrics
                                .record_no_damage(render_started.elapsed(), missed_deadlines);
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
    for window in state.space.elements() {
        window.send_frame(output, time, Some(refresh), |_, _| Some(output.clone()));
    }
    for layer in layer_surfaces(output) {
        layer.send_frame(output, time, Some(refresh), |_, _| Some(output.clone()));
    }
    state.send_cursor_frame(output);
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
    FrameEffectMetrics
}

fn output_elements(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
    include_cursor: bool,
) -> Vec<AnimatedWindowRenderElement> {
    if state.session_lock.active {
        state.configure_lock_surfaces();
        let Some(geometry) = state.space.output_geometry(output) else {
            return Vec::new();
        };
        let scale = output.current_scale().fractional_scale();
        let background = state
            .session_lock
            .backgrounds
            .entry(output.clone())
            .or_insert_with(|| {
                smithay::backend::renderer::element::solid::SolidColorBuffer::new(
                    geometry.size,
                    [0.0, 0.0, 0.0, 1.0],
                )
            });
        background.resize(geometry.size);
        let mut elements = Vec::new();
        let opacity = state.session_lock.idle_opacity;
        if opacity > 0.0 {
            let overlay = state
                .session_lock
                .idle_overlays
                .entry(output.clone())
                .or_insert_with(|| {
                    smithay::backend::renderer::element::solid::SolidColorBuffer::new(
                        geometry.size,
                        [0.0, 0.0, 0.0, 1.0],
                    )
                });
            overlay.resize(geometry.size);
            elements.push(
                SolidColorRenderElement::from_buffer(
                    overlay,
                    (0, 0),
                    scale,
                    opacity,
                    RenderElementKind::Unspecified,
                )
                .into(),
            );
        }
        if let Some(surface) = state
            .session_lock
            .surfaces
            .get(output)
            .filter(|surface| surface.alive())
        {
            elements.extend(
                render_elements_from_surface_tree::<
                    GlesRenderer,
                    WaylandSurfaceRenderElement<GlesRenderer>,
                >(
                    renderer,
                    surface.wl_surface(),
                    (0, 0),
                    scale,
                    1.0,
                    RenderElementKind::Unspecified,
                )
                .into_iter()
                .map(AnimatedWindowRenderElement::from),
            );
        }
        elements.push(
            SolidColorRenderElement::from_buffer(
                background,
                (0, 0),
                scale,
                1.0,
                RenderElementKind::Unspecified,
            )
            .into(),
        );
        return elements;
    }
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
    if state.overview.is_presenting() {
        elements.extend(overview_strip_elements(
            state,
            renderer,
            output,
            output_geometry,
            scale,
        ));
    }
    let mut candidates = if state.overview.is_active() {
        state.window_ids.keys().cloned().collect::<Vec<_>>()
    } else {
        state.space.elements().rev().cloned().collect::<Vec<_>>()
    };
    if state.overview.is_active() {
        candidates.sort_by_key(|window| {
            std::cmp::Reverse(state.window_ids.get(window).map_or(0, |id| id.0))
        });
    }
    let windows = candidates
        .iter()
        .filter_map(|window| {
            // Configure immediately, but present the frame/shadow only after
            // the first buffer commit has settled the actual client geometry.
            if !state.window_content_ready(window) {
                return None;
            }
            let id = *state.window_ids.get(window)?;
            // Scrolling columns may sit outside their monitor's rectangle.
            // They must not reappear on a neighboring output just because
            // their global animated coordinates overlap it.
            if !state.window_belongs_to_output(id, output) {
                return None;
            }
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
        let pixels = physical_rect(visual, output_geometry.loc, scale);
        let material_surface = window
            .toplevel()
            .map(|toplevel| toplevel.wl_surface())
            .filter(|surface| {
                crate::effects::surface_role(surface)
                    .is_some_and(|(role, _)| role == crate::effects::SemanticRole::Modal)
            });
        let window_radius = if material_surface.is_some() {
            state.theme_settings.material_radius
        } else {
            state.theme_settings.window_radius
        } * decoration_progress;
        // Only overview/close intentionally scale the complete application.
        let scale_content = state.overview.is_presenting() || state.closing_visual(id).0 != 1.0;
        let behavior = resize_content_behavior(scale_content);

        let dim = state.window_dimming.get(&id).map_or(0.0, |dim| dim.current);
        if let Some(overlay) = window_tint_element(
            state,
            renderer,
            id,
            constrain,
            pixels,
            scale,
            window_radius,
            [0.0, 0.0, 0.0, dim as f32 * close_alpha],
            false,
            output,
        ) {
            // Front-to-back: dim the application and its border, not its shadow
            // or other windows/layer surfaces. The overlay is input-transparent.
            elements.push(overlay.into());
        }

        if let Some(programs) = rounded_clip_program.clone() {
            let shadow_offset_y = state.theme_settings.shadow_offset_y;
            let shadow_blur = state.theme_settings.shadow_blur;
            let shadow_opacity = state.theme_settings.shadow_opacity;
            let shadow_color = state.theme_settings.shadow_color.0;
            let focused = if state.overview.is_active() {
                state.overview_selected(id)
            } else {
                state.focused_window == Some(id)
            };
            let focus = state
                .window_focus
                .get(&id)
                .map_or(if focused { 1.0 } else { 0.0 }, |v| v.current);
            let border_width = state.theme_settings.border_width
                + (state.theme_settings.focus_ring_width - state.theme_settings.border_width)
                    * focus;
            let border_color = state.theme_settings.border_color.0;
            let gradient = state.theme_settings.border_gradient;

            if let Some(border) = window_border_element(
                state,
                renderer,
                id,
                constrain,
                pixels,
                scale,
                window_radius,
                border_width,
                border_color,
                gradient,
                focus as f32,
                close_alpha * decoration_progress as f32,
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
                pixels,
                scale,
                window_radius,
                shadow_offset_y,
                shadow_blur,
                shadow_opacity * f64::from(close_alpha) * decoration_progress,
                shadow_color,
                output,
                &programs,
            );
            if !scale_content
                && let Some(snapshot) = state.resize_snapshots.get(&id)
                && snapshot.context == renderer.context_id().erased()
                && (snapshot.scale - scale).abs() < 0.001
            {
                let size = snapshot.texture.size();
                let visible =
                    Rectangle::new(pixels.loc, (size.w, size.h).into()).intersection(pixels);
                if let Some(visible) = visible {
                    let clip = framebuffer_clip_rect(
                        pixels,
                        output.current_mode().unwrap().size,
                        output.current_transform().invert(),
                    );
                    elements.push(
                        NativeTextureElement {
                            id: snapshot.id.clone(),
                            commit: snapshot.commit,
                            texture: snapshot.texture.clone(),
                            geometry: visible,
                            source: Rectangle::from_size(Size::from((
                                f64::from(visible.size.w),
                                f64::from(visible.size.h),
                            ))),
                            alpha: crate::presentation::handoff_alpha(snapshot.elapsed)
                                * close_alpha,
                            program: Some(programs.texture.clone()),
                            uniforms: vec![
                                Uniform::new("clip_rect", clip).into_owned(),
                                Uniform::new(
                                    "radius",
                                    scaled_effect_value(window_radius, constrain, scale),
                                )
                                .into_owned(),
                            ],
                        }
                        .into(),
                    );
                }
            }
            elements.extend(rounded_window_elements(
                renderer,
                &window,
                constrain,
                pixels,
                scale,
                window_radius,
                close_alpha,
                state
                    .window_geometry
                    .get(&id)
                    .is_some_and(|geometry| geometry.presentation_changed),
                output,
                programs.texture.clone(),
                behavior,
            ));
            let source = window.geometry().size;
            if !scale_content && (source.w < constrain.size.w || source.h < constrain.size.h) {
                let mut color = state.theme_settings.surface_base_color.0;
                color[3] = close_alpha;
                if let Some(fill) = window_tint_element(
                    state,
                    renderer,
                    id,
                    constrain,
                    pixels,
                    scale,
                    window_radius,
                    color,
                    true,
                    output,
                ) {
                    // Front-to-back: fill uncovered strips behind the native
                    // content instead of stretching it or exposing wallpaper.
                    elements.push(fill.into());
                }
            } else {
                state.window_resize_fills.remove(&id);
            }
            if let Some(surface) = material_surface
                && let Some((background, _)) = material_element(
                    state,
                    renderer,
                    output,
                    surface,
                    constrain,
                    window_radius as f32,
                    0,
                    constrain,
                    close_alpha,
                )
            {
                elements.push(background);
            }
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
                    behavior,
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
    if let Some(wallpaper) = state.wallpaper.element(renderer, output) {
        elements.push(wallpaper.into());
    }
    elements
}

#[allow(clippy::too_many_arguments)]
fn overview_chrome_element(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    scale: f64,
    key: (u64, OverviewChromePart),
    rect: ferese_layout::Rect,
    color: [f32; 4],
    width: f64,
    radius: f64,
) -> Option<PhysicalShaderElement> {
    let output_id = state.output_id(output)?;
    let geometry = rounded_visual_rect(rect, output_geometry.loc);
    let physical = physical_rect(rect, output_geometry.loc, scale);
    let mode = output.current_mode()?;
    let parameters = BorderParameters {
        geometry,
        clip_rect: framebuffer_clip_rect(physical, mode.size, output.current_transform().invert()),
        radius: (radius * scale) as f32,
        width: (width * scale) as f32,
        color,
        color_to: color,
        gradient_line: [0.0; 4],
        focus_color: color,
        focus_color_to: color,
        focus_gradient_line: [0.0; 4],
        focus_mix: 0.0,
    };
    let program = if key.1 == OverviewChromePart::Outline {
        rounded_clip_program(state, renderer)?.border
    } else {
        material_program(state, renderer)?.0
    };
    let uniforms = |p: &BorderParameters| {
        if key.1 == OverviewChromePart::Outline {
            border_uniforms(p)
        } else {
            vec![
                Uniform::new("visible_rect", p.clip_rect).into_owned(),
                Uniform::new("material_radius", p.radius).into_owned(),
                Uniform::new("tint", p.color).into_owned(),
                Uniform::new("paint_mode", 0.0_f32).into_owned(),
                Uniform::new("shadow_rect", p.clip_rect).into_owned(),
                Uniform::new("shadow_values", [0.0_f32; 2]).into_owned(),
            ]
        }
    };
    let context = renderer.context_id().erased();
    let chrome = &mut state
        .overview_scrims
        .entry(output_id)
        .or_insert_with(|| OverviewScrim {
            chrome: HashMap::new(),
        })
        .chrome;
    let cached = chrome
        .entry(key)
        .or_default()
        .entry(context)
        .or_insert_with(|| CachedBorder {
            element: PixelShaderElement::new(
                program,
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
        cached.element.update_uniforms(uniforms(&parameters));
        cached.parameters = parameters;
    }
    Some(PhysicalShaderElement {
        inner: cached.element.clone(),
        geometry: physical,
    })
}

fn overview_strip_elements(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
    output_geometry: Rectangle<i32, Logical>,
    scale: f64,
) -> Vec<AnimatedWindowRenderElement> {
    let alpha = state.overview.opacity();
    if alpha <= 0.001 {
        return Vec::new();
    }
    let Some(bounds) = state.output_bounds_for(output) else {
        return Vec::new();
    };
    let cards = state.overview_workspace_cards(output);
    let strip = crate::overview::workspace_strip(bounds);
    let panel = match (cards.first(), cards.last()) {
        (Some(first), Some(last)) => ferese_layout::Rect::new(
            first.rect.x - 12.0,
            strip.y,
            last.rect.x + last.rect.width - first.rect.x + 24.0,
            strip.height,
        ),
        _ => strip,
    };
    let radius = state.theme_settings.material_radius;
    let material_opacity = crate::effects::resolve_material(
        crate::effects::SemanticRole::Panel,
        state.theme_settings.material_style,
        state.theme_settings.shell_opacity as f32,
    )
    .opacity;
    let mut elements = Vec::new();
    let programs = rounded_clip_program(state, renderer);
    // Resolve IDs once per strip, rather than scanning all managed windows
    // separately for every miniature. Own the handles so rendering can mutate state.
    let windows_by_id: std::collections::HashMap<_, _> = state
        .window_ids
        .iter()
        .map(|(window, id)| (*id, window.clone()))
        .collect();
    if let Some(output_id) = state.output_id(output)
        && let Some(cache) = state.overview_scrims.get_mut(&output_id)
    {
        cache.chrome.retain(|(id, part), _| match part {
            OverviewChromePart::Caption => windows_by_id.keys().any(|window| window.0 == *id),
            OverviewChromePart::Card | OverviewChromePart::Outline => {
                cards.iter().any(|card| card.workspace.0 == *id)
            }
            OverviewChromePart::Strip => true,
        });
    }
    // Cached title textures are independent of window content and stay readable
    // when thumbnails are small. Their pills use the shell's material and radius.
    for (id, window) in &windows_by_id {
        if !state.window_belongs_to_output(*id, output) {
            continue;
        }
        let Some(rect) = state.presented_window_rect(*id) else {
            continue;
        };
        let Some((buffer, size)) = state.overview_window_label(window, scale, rect.width) else {
            continue;
        };
        let width = (f64::from(size.w) / scale + 16.0).min(rect.width);
        let caption = ferese_layout::Rect::new(
            rect.x + (rect.width - width) * 0.5,
            rect.y + rect.height + 8.0,
            width,
            26.0,
        );
        let location = Point::<i32, Physical>::from((
            ((caption.x - f64::from(output_geometry.loc.x) + caption.width * 0.5) * scale).round()
                as i32
                - size.w / 2,
            ((caption.y - f64::from(output_geometry.loc.y) + 4.0) * scale).round() as i32,
        ));
        if let Ok(element) = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            location.to_f64(),
            &buffer,
            Some(alpha),
            Some(Rectangle::from_size(
                (f64::from(size.w), f64::from(size.h)).into(),
            )),
            Some(
                (
                    (f64::from(size.w) / scale).round() as i32,
                    (f64::from(size.h) / scale).round() as i32,
                )
                    .into(),
            ),
            RenderElementKind::Unspecified,
        ) {
            let element = RescaleRenderElement::from_element(element, location, 1.0);
            let element = RelocateRenderElement::from_element(element, (0, 0), Relocate::Relative);
            if let Some(element) = CropRenderElement::from_element(
                element,
                scale,
                physical_rect(caption, output_geometry.loc, scale),
            ) {
                elements.push(element.into());
            }
        }
        let mut fill = state.theme_settings.surface_base_color.0;
        fill[3] = alpha * material_opacity;
        if let Some(background) = overview_chrome_element(
            state,
            renderer,
            output,
            output_geometry,
            scale,
            (id.0, OverviewChromePart::Caption),
            caption,
            fill,
            0.0,
            radius.min(13.0),
        ) {
            elements.push(background.into());
        }
    }
    for card in cards {
        // Front-to-back: outline and live miniatures above each card, all above
        // the strip. Nothing is painted behind the main overview window grid.
        if card.selected {
            let mut color = state.theme_settings.accent_color.0;
            color[3] *= alpha;
            if let Some(border) = overview_chrome_element(
                state,
                renderer,
                output,
                output_geometry,
                scale,
                (card.workspace.0, OverviewChromePart::Outline),
                card.rect,
                color,
                1.0,
                radius,
            ) {
                elements.push(border.into());
            }
        }
        if let Some(programs) = &programs {
            for (id, rect) in &card.windows {
                if let Some(window) = windows_by_id.get(id) {
                    elements.extend(rounded_window_elements(
                        renderer,
                        window,
                        rounded_visual_rect(*rect, output_geometry.loc),
                        physical_rect(*rect, output_geometry.loc, scale),
                        scale,
                        4.0,
                        alpha,
                        true,
                        output,
                        programs.texture.clone(),
                        ConstrainScaleBehavior::Stretch,
                    ));
                }
            }
        }
        if let Some((buffer, size)) = state.overview_workspace_label(card.workspace, scale) {
            let location = Point::<i32, Physical>::from((
                ((card.rect.x - f64::from(output_geometry.loc.x) + card.rect.width * 0.5) * scale)
                    .round() as i32
                    - size.w / 2,
                ((card.rect.y - f64::from(output_geometry.loc.y) + card.rect.height - 20.0) * scale)
                    .round() as i32,
            ));
            if let Ok(element) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                location.to_f64(),
                &buffer,
                Some(alpha),
                Some(Rectangle::from_size(
                    (f64::from(size.w), f64::from(size.h)).into(),
                )),
                Some(
                    (
                        (f64::from(size.w) / scale).round() as i32,
                        (f64::from(size.h) / scale).round() as i32,
                    )
                        .into(),
                ),
                RenderElementKind::Unspecified,
            ) {
                let element = RescaleRenderElement::from_element(element, location, 1.0);
                let element =
                    RelocateRenderElement::from_element(element, (0, 0), Relocate::Relative);
                if let Some(element) = CropRenderElement::from_element(
                    element,
                    scale,
                    physical_rect(card.rect, output_geometry.loc, scale),
                ) {
                    elements.push(element.into());
                }
            }
        }
        let mut card_color = state.theme_settings.surface_base_color.0;
        for component in &mut card_color[..3] {
            *component = (*component + 0.035).min(1.0);
        }
        card_color[3] = alpha * material_opacity;
        if let Some(card_fill) = overview_chrome_element(
            state,
            renderer,
            output,
            output_geometry,
            scale,
            (card.workspace.0, OverviewChromePart::Card),
            card.rect,
            card_color,
            0.0,
            radius,
        ) {
            elements.push(card_fill.into());
        }
    }
    let mut color = state.theme_settings.surface_base_color.0;
    color[3] = alpha
        * crate::effects::resolve_material(
            crate::effects::SemanticRole::Panel,
            state.theme_settings.material_style,
            state.theme_settings.shell_opacity as f32,
        )
        .opacity;
    if let Some(strip) = overview_chrome_element(
        state,
        renderer,
        output,
        output_geometry,
        scale,
        (0, OverviewChromePart::Strip),
        panel,
        color,
        0.0,
        radius,
    ) {
        elements.push(strip.into());
    }
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
        UniformName::new("border_color_to", UniformType::_4f),
        UniformName::new("gradient_line", UniformType::_4f),
        UniformName::new("focus_color", UniformType::_4f),
        UniformName::new("focus_color_to", UniformType::_4f),
        UniformName::new("focus_gradient_line", UniformType::_4f),
        UniformName::new("focus_mix", UniformType::_1f),
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
    physical: Rectangle<i32, Physical>,
    scale: f64,
    requested_radius: f64,
    requested_width: f64,
    color: [f32; 4],
    gradient: Option<crate::config::BorderGradient>,
    focus_mix: f32,
    opacity: f32,
    output: &Output,
    programs: &RoundedClipPrograms,
) -> Option<PhysicalShaderElement> {
    let mode = output.current_mode()?;
    let width = scaled_effect_value(requested_width, geometry, scale);
    if width == 0.0 {
        return None;
    }

    let (from, to, gradient_line) = match gradient {
        Some(gradient) => (
            gradient.from.0,
            gradient.to.0,
            border_gradient_line(
                physical,
                mode.size,
                output.current_transform().invert(),
                gradient.angle,
            ),
        ),
        None => (color, color, [0.0; 4]),
    };
    let (focus_from, focus_to, focus_gradient_line) = match state.theme_settings.focus_ring_gradient
    {
        Some(g) => (
            g.from.0,
            g.to.0,
            border_gradient_line(
                physical,
                mode.size,
                output.current_transform().invert(),
                g.angle,
            ),
        ),
        None => (
            state.theme_settings.accent_color.0,
            state.theme_settings.accent_color.0,
            [0.0; 4],
        ),
    };
    let parameters = BorderParameters {
        geometry,
        clip_rect: framebuffer_clip_rect(physical, mode.size, output.current_transform().invert()),
        radius: scaled_effect_value(requested_radius, geometry, scale),
        width,
        color: color_with_alpha(from, opacity),
        color_to: color_with_alpha(to, opacity),
        gradient_line,
        focus_color: color_with_alpha(focus_from, opacity),
        focus_color_to: color_with_alpha(focus_to, opacity),
        focus_gradient_line,
        focus_mix,
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

    Some(PhysicalShaderElement {
        inner: cached.element.clone(),
        geometry: physical,
    })
}

fn border_uniforms(parameters: &BorderParameters) -> Vec<Uniform<'static>> {
    vec![
        Uniform::new("clip_rect", parameters.clip_rect).into_owned(),
        Uniform::new("radius", parameters.radius).into_owned(),
        Uniform::new("border_width", parameters.width).into_owned(),
        Uniform::new("border_color", parameters.color).into_owned(),
        Uniform::new("border_color_to", parameters.color_to).into_owned(),
        Uniform::new("gradient_line", parameters.gradient_line).into_owned(),
        Uniform::new("focus_color", parameters.focus_color).into_owned(),
        Uniform::new("focus_color_to", parameters.focus_color_to).into_owned(),
        Uniform::new("focus_gradient_line", parameters.focus_gradient_line).into_owned(),
        Uniform::new("focus_mix", parameters.focus_mix).into_owned(),
    ]
}

#[allow(clippy::too_many_arguments)]
fn window_tint_element(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    id: ferese_layout::WindowId,
    geometry: Rectangle<i32, Logical>,
    physical: Rectangle<i32, Physical>,
    scale: f64,
    radius: f64,
    color: [f32; 4],
    resize_fill: bool,
    output: &Output,
) -> Option<PhysicalShaderElement> {
    if color[3] <= 0.0 {
        if resize_fill {
            state.window_resize_fills.remove(&id);
        } else {
            state.window_dims.remove(&id);
        }
        return None;
    }
    let mode = output.current_mode()?;
    let program = material_program(state, renderer)?;
    let parameters = BorderParameters {
        geometry,
        clip_rect: framebuffer_clip_rect(physical, mode.size, output.current_transform().invert()),
        radius: scaled_effect_value(radius, geometry, scale),
        width: 0.0,
        color,
        color_to: color,
        gradient_line: [0.0; 4],
        focus_color: color,
        focus_color_to: color,
        focus_gradient_line: [0.0; 4],
        focus_mix: 0.0,
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
    let buffers = if resize_fill {
        state.window_resize_fills.entry(id).or_default()
    } else {
        state.window_dims.entry(id).or_default()
    };
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
    Some(PhysicalShaderElement {
        inner: cached.element.clone(),
        geometry: physical,
    })
}

#[allow(clippy::too_many_arguments)]
fn window_shadow_element(
    state: &mut Ferese,
    renderer: &GlesRenderer,
    id: ferese_layout::WindowId,
    geometry: Rectangle<i32, Logical>,
    physical: Rectangle<i32, Physical>,
    scale: f64,
    requested_radius: f64,
    offset_y: f64,
    blur: f64,
    opacity: f64,
    color: [f32; 4],
    output: &Output,
    programs: &RoundedClipPrograms,
) -> Option<PhysicalShaderElement> {
    let mode = output.current_mode()?;
    if opacity == 0.0 || color[3] == 0.0 {
        return None;
    }

    let shadow_geometry = Rectangle::new(
        (
            physical.loc.x,
            physical.loc.y + (offset_y * scale).round() as i32,
        )
            .into(),
        physical.size,
    );
    let bounds = shadow_bounds(geometry, offset_y, blur);
    let parameters = ShadowParameters {
        blur: (blur * scale) as f32,
        bounds,
        shadow_rect: framebuffer_clip_rect(
            shadow_geometry,
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

    let extent = (blur * scale * 2.0).ceil() as i32;
    Some(PhysicalShaderElement {
        inner: cached.element.clone(),
        geometry: Rectangle::new(
            (
                shadow_geometry.loc.x - extent,
                shadow_geometry.loc.y - extent,
            )
                .into(),
            (
                shadow_geometry.size.w + 2 * extent,
                shadow_geometry.size.h + 2 * extent,
            )
                .into(),
        ),
    })
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
    physical_constrain: Rectangle<i32, Physical>,
    scale: f64,
    requested_radius: f64,
    alpha: f32,
    clip_changed: bool,
    output: &Output,
    program: GlesTexProgram,
    behavior: ConstrainScaleBehavior,
) -> Vec<AnimatedWindowRenderElement> {
    let Some(toplevel) = window.toplevel() else {
        return Vec::new();
    };
    let Some(mode) = output.current_mode() else {
        return Vec::new();
    };

    let geometry = window.geometry();
    let reference = geometry.to_physical_precise_round(scale);
    let location = physical_constrain.loc - geometry.loc.to_physical_precise_round(scale);
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
        behavior,
        ConstrainAlign::TOP | ConstrainAlign::LEFT,
        scale,
    )
    .map(Into::into)
    .collect()
}

fn resize_content_behavior(intentional_scale: bool) -> ConstrainScaleBehavior {
    if intentional_scale {
        ConstrainScaleBehavior::Stretch
    } else {
        ConstrainScaleBehavior::CutOff
    }
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

fn border_gradient_line(
    geometry: Rectangle<i32, Physical>,
    output_size: Size<i32, Physical>,
    transform: Transform,
    angle: f64,
) -> [f32; 4] {
    let angle = angle.to_radians();
    let direction = (angle.cos(), angle.sin());
    let width = f64::from(geometry.size.w);
    let height = f64::from(geometry.size.h);
    let extent = (width * direction.0.abs() + height * direction.1.abs()) * 0.5;
    let center = geometry.loc.to_f64() + Point::from((width * 0.5, height * 0.5));
    let offset: Point<f64, Physical> = (direction.0 * extent, direction.1 * extent).into();
    // Apply the same output-to-framebuffer transform as the rounded clip.
    // This keeps angles in window coordinates on rotated/flipped monitors.
    let area = transform.transform_size(output_size).to_f64();
    let start = transform.transform_point_in(center - offset, &area);
    let end = transform.transform_point_in(center + offset, &area);
    let delta = end - start;
    let length_squared = (delta.x * delta.x + delta.y * delta.y).max(0.000001);
    [
        start.x as f32,
        start.y as f32,
        (delta.x / length_squared) as f32,
        (delta.y / length_squared) as f32,
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
                state, renderer, output, surface, *rect, *radius, index, geometry, 1.0,
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
        crate::effects::surface_opacity(surface),
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
    alpha: f32,
) -> Option<(AnimatedWindowRenderElement, AnimatedWindowRenderElement)> {
    let (role, generation) = crate::effects::surface_role(surface)?;
    let material = crate::effects::resolve_material(
        role,
        state.theme_settings.material_style,
        state.theme_settings.shell_opacity as f32,
    );
    let presentation_alpha = crate::effects::surface_opacity(surface) * alpha;
    let mode = output.current_mode()?;
    let scale = output.current_scale().fractional_scale();
    let transform = output.current_transform().invert();
    let [red, green, blue, _] = state.theme_settings.surface_base_color.0;
    let radius = radius.min(geometry.size.w.min(geometry.size.h) as f32 * 0.5);
    let [offset_y, shadow_blur, shadow_opacity] = material.shadow;
    let edge_bar =
        role == crate::effects::SemanticRole::Panel && radius == 0.0 && geometry.loc.y == 0;
    let shadow_opacity = if edge_bar {
        0.0
    } else {
        shadow_opacity * f64::from(presentation_alpha)
    };
    let shadow_geometry = Rectangle::new(
        (geometry.loc.x, geometry.loc.y + offset_y.round() as i32).into(),
        geometry.size,
    );
    let background_opacity = material.opacity;
    let blur = material_blur_radius(
        material.style,
        material.opacity,
        state.theme_settings.backdrop_blur,
    );
    let output_size = output
        .current_transform()
        .transform_size(mode.size)
        .to_f64()
        .to_logical(scale)
        .to_i32_ceil();
    let sample_geometry = expanded_blur_region(capture_geometry, blur.ceil() as i32, output_size);
    let sample_physical = sample_geometry.to_physical_precise_round(scale);
    let mut parameters = MaterialParameters {
        presentation_alpha,
        background_opacity,
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
        tint: [red, green, blue, material.opacity * presentation_alpha],
        generation,
        opaque: material.opacity == 1.0 && presentation_alpha == 1.0 && radius == 0.0,
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
    let capture_rect = framebuffer_capture_rect(parameters.sample_framebuffer);
    let capture_size = Size::from((capture_rect[2], capture_rect[3]));
    let buffers = state.material_buffers.entry(surface.clone()).or_default();
    let capture = if let Some(blur_program) = blur_program {
        if buffers
            .captures
            .get(&context)
            .is_none_or(|c| c.geometry != sample_geometry || c.texture.size() != capture_size)
        {
            let texture = Offscreen::<GlesTexture>::create_buffer(
                renderer,
                Fourcc::Abgr8888,
                capture_size,
            )
            .map_err(|error| {
                tracing::warn!(%error, ?sample_physical, "backdrop capture allocation failed");
                error
            })
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
    if capture.is_some() {
        parameters.tint[3] = BLURRED_MATERIAL_TINT;
    }
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

fn material_blur_radius(style: crate::config::MaterialStyle, opacity: f32, radius: f64) -> f64 {
    if style == crate::config::MaterialStyle::Translucent && opacity > 0.0 {
        radius
    } else {
        0.0
    }
}

fn blur_program(state: &mut Ferese, renderer: &mut GlesRenderer) -> Option<BlurProgram> {
    let context = renderer.context_id().erased();
    if let Some(program) = state.blur_programs.get(&context) {
        return Some(program.clone());
    }
    match renderer.compile_custom_texture_shader(BLUR_SHADER, &blur_uniform_names()) {
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

fn blur_uniform_names() -> [UniformName<'static>; 8] {
    [
        UniformName::new("visible_rect", UniformType::_4f),
        UniformName::new("material_radius", UniformType::_1f),
        UniformName::new("texture_size", UniformType::_2f),
        UniformName::new("capture_origin", UniformType::_2f),
        UniformName::new("blur_radius", UniformType::_1f),
        UniformName::new("presentation_alpha", UniformType::_1f),
        UniformName::new("background_opacity", UniformType::_1f),
        UniformName::new("tint", UniformType::_4f),
    ]
}

fn blur_uniforms(p: &MaterialParameters) -> Vec<Uniform<'static>> {
    vec![
        Uniform::new("visible_rect", p.visible_framebuffer).into_owned(),
        Uniform::new("material_radius", p.radius).into_owned(),
        Uniform::new(
            "texture_size",
            [p.sample_framebuffer[2], p.sample_framebuffer[3]],
        )
        .into_owned(),
        Uniform::new(
            "capture_origin",
            [p.sample_framebuffer[0], p.sample_framebuffer[1]],
        )
        .into_owned(),
        Uniform::new("blur_radius", p.blur).into_owned(),
        Uniform::new("presentation_alpha", p.presentation_alpha).into_owned(),
        Uniform::new("background_opacity", p.background_opacity).into_owned(),
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
    if state.input_capture.active() {
        return Vec::new();
    }
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
        border_gradient_line, color_with_alpha, framebuffer_clip_rect, normalized_scale,
        resize_content_behavior, rounded_visual_rect, scaled_visual_rect, shadow_bounds,
    };

    #[test]
    #[ignore = "requires an EGL rendering device"]
    fn translucent_blur_does_not_leak_the_sharp_backdrop() {
        use smithay::backend::{
            allocator::Fourcc,
            egl::{EGLContext, EGLDevice, EGLDisplay},
            renderer::{
                Bind, Color32F, ExportMem, Frame, ImportMem, Offscreen, Renderer,
                gles::{GlesRenderer, GlesTexture, Uniform},
            },
        };

        let device = EGLDevice::enumerate()
            .unwrap()
            .last()
            .expect("an EGL device");
        let display = unsafe { EGLDisplay::new(device).unwrap() };
        let context = EGLContext::new(&display).unwrap();
        let mut renderer = unsafe { GlesRenderer::new(context).unwrap() };
        let program = renderer
            .compile_custom_texture_shader(super::BLUR_SHADER, &super::blur_uniform_names())
            .unwrap();
        let size = (32, 32).into();
        let mut pixels = vec![128u8; 32 * 32 * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let blurred = renderer
            .import_memory(&pixels, Fourcc::Abgr8888, size, false)
            .unwrap();
        let mut target_texture: GlesTexture =
            renderer.create_buffer(Fourcc::Abgr8888, size).unwrap();
        let damage = Rectangle::<i32, Physical>::from_size((32, 32).into());

        for opacity in [0.2f32, 0.71, 1.0] {
            let uniforms = vec![
                Uniform::new("visible_rect", [0.0f32, 0.0, 32.0, 32.0]),
                Uniform::new("material_radius", 0.0f32),
                Uniform::new("texture_size", [32.0f32, 32.0]),
                Uniform::new("capture_origin", [0.0f32, 0.0]),
                Uniform::new("blur_radius", 12.0f32),
                Uniform::new("presentation_alpha", 1.0f32),
                Uniform::new("background_opacity", opacity),
                Uniform::new("tint", [0.1f32, 0.1, 0.1, super::BLURRED_MATERIAL_TINT]),
            ];
            let mut target = renderer.bind(&mut target_texture).unwrap();
            {
                let mut frame = renderer
                    .render(&mut target, (32, 32).into(), Transform::Normal)
                    .unwrap();
                frame
                    .clear(Color32F::new(0.0, 0.0, 0.0, 1.0), &[damage])
                    .unwrap();
                frame
                    .draw_solid(
                        Rectangle::new((0, 0).into(), (16, 32).into()),
                        &[damage],
                        Color32F::new(1.0, 1.0, 1.0, 1.0),
                    )
                    .unwrap();
                frame
                    .render_texture_from_to(
                        &blurred,
                        Rectangle::from_size(size.to_f64()),
                        damage,
                        &[damage],
                        &[],
                        Transform::Normal,
                        1.0,
                        Some(&program),
                        &uniforms,
                    )
                    .unwrap();
                let _ = frame.finish().unwrap();
            }
            let mapping = renderer
                .copy_framebuffer(&target, Rectangle::from_size(size), Fourcc::Abgr8888)
                .unwrap();
            let rendered = renderer.map_texture(&mapping).unwrap();
            let left = rendered[(16 * 32 + 8) * 4];
            let right = rendered[(16 * 32 + 24) * 4];
            assert!(
                left.abs_diff(right) <= 2,
                "opacity {opacity} leaked the original backdrop: {left} versus {right}"
            );
            let expected = (128.0 * (1.0 - super::BLURRED_MATERIAL_TINT * opacity)
                + 25.5 * super::BLURRED_MATERIAL_TINT * opacity)
                .round() as u8;
            assert!(
                left.abs_diff(expected) <= 2,
                "opacity {opacity} failed to control tint: {left} versus {expected}"
            );
        }
    }

    #[test]
    fn gradient_direction_survives_every_output_transform() {
        let mode = (2560, 1600).into();
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
            let area = transform.transform_size(mode).to_f64();
            let rect: Rectangle<i32, Physical> =
                Rectangle::new((110, 60).into(), (800, 600).into());
            for (angle, first, last) in [
                (0.0, (110.0, 360.0), (910.0, 360.0)),
                (90.0, (510.0, 60.0), (510.0, 660.0)),
                (45.0, (110.0, 60.0), (910.0, 660.0)),
                (135.0, (910.0, 60.0), (110.0, 660.0)),
            ] {
                let line = border_gradient_line(rect, mode, transform, angle);
                let progress = |point: smithay::utils::Point<f64, Physical>| {
                    let point = transform.transform_point_in(point, &area);
                    (point.x - f64::from(line[0])) * f64::from(line[2])
                        + (point.y - f64::from(line[1])) * f64::from(line[3])
                };
                assert!(
                    progress(first.into()).abs() < 0.00001,
                    "{transform:?}, {angle}"
                );
                assert!(
                    (progress(last.into()) - 1.0).abs() < 0.00001,
                    "{transform:?}, {angle}"
                );
            }
        }
    }

    #[test]
    fn gradient_coordinates_follow_fractional_resize_and_translation() {
        for scale in [1.0, 1.25, 1.5, 1.8, 2.0] {
            for width in [500.0, 733.25, 1000.0] {
                let rect = crate::presentation::physical_rect(
                    ferese_layout::Rect::new(40.25, 60.75, width, 600.5),
                    (0, 0).into(),
                    scale,
                );
                let line = border_gradient_line(rect, (3840, 2160).into(), Transform::Normal, 0.0);
                assert!((f64::from(line[0]) - f64::from(rect.loc.x)).abs() < 0.001);
                assert!((f64::from(line[2]) * f64::from(rect.size.w) - 1.0).abs() < 0.00001);
                assert!(line.iter().all(|component| component.is_finite()));
            }
        }
    }

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
    fn resize_keeps_source_pixels_native_when_growing_and_shrinking() {
        use smithay::backend::renderer::element::utils::{
            ConstrainAlign, constrain_render_elements,
        };
        for scale in [1.0, 1.5, 1.8, 2.0] {
            for width in [300, 600, 1200] {
                let source = Rectangle::<i32, Logical>::from_size((600, 800).into());
                let destination = Rectangle::<i32, Logical>::from_size((width, 800).into());
                let element = constrain_render_elements(
                    [DamageElement::new(source)],
                    (0, 0),
                    destination.to_physical_precise_round(scale),
                    source.to_physical_precise_round(scale),
                    resize_content_behavior(false),
                    ConstrainAlign::TOP | ConstrainAlign::LEFT,
                    scale,
                )
                .next()
                .unwrap();
                // Comparing destination pixels to sampled source pixels catches
                // stretching, unlike tests of configure timing alone.
                let physical = element.geometry(scale.into());
                let sampled = element.src();
                assert!((f64::from(physical.size.w) / sampled.size.w - scale).abs() < 0.01);
                assert!((f64::from(physical.size.h) / sampled.size.h - scale).abs() < 0.01);
            }
        }
    }

    #[test]
    fn overview_retains_intentional_content_scaling() {
        assert!(matches!(
            resize_content_behavior(true),
            smithay::backend::renderer::element::utils::ConstrainScaleBehavior::Stretch
        ));
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
    fn translucent_materials_blur_at_full_opacity() {
        use crate::config::MaterialStyle;
        assert_eq!(
            super::material_blur_radius(MaterialStyle::Translucent, 0.0, 18.0),
            0.0
        );
        assert_eq!(
            super::material_blur_radius(MaterialStyle::Translucent, 1.0, 18.0),
            18.0
        );
        assert_eq!(
            super::material_blur_radius(MaterialStyle::Translucent, 0.79, 18.0),
            18.0
        );
        assert_eq!(
            super::material_blur_radius(MaterialStyle::Solid, 0.79, 18.0),
            0.0
        );
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
