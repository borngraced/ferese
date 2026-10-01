mod capture;
mod decorations;
mod materials;
mod overview;
mod scene;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub(crate) use capture::{capture_resize_snapshot, capture_window_buffer, capture_window_frame};
use decorations::*;
use materials::*;
use overview::*;
use scene::cursor_elements;
pub(crate) use scene::{
    animated_window_elements, cursorless_window_elements, frame_effect_metrics, layer_surfaces, output_elements,
};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, WaylandSurfaceTexture, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::utils::{
    ConstrainAlign, ConstrainScaleBehavior, CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement,
    constrain_render_elements,
};
use smithay::backend::renderer::element::{
    Element, Id, Kind as RenderElementKind, RenderElement, UnderlyingStorage, render_elements,
};
use smithay::backend::renderer::gles::element::PixelShaderElement;
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType,
};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions, draw_render_elements};
use smithay::backend::renderer::{Bind, Color32F, ErasedContextId, ExportMem, Frame, Offscreen, Renderer, Texture};
use smithay::desktop::space::{ConstrainBehavior, ConstrainReference, constrain_space_element};
use smithay::desktop::{LayerSurface, PopupManager, layer_map_for_output};
use smithay::input::pointer::{CursorImageStatus, CursorImageSurfaceData};
use smithay::output::Output;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale as RenderScale, Size, Transform};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::wlr_layer::Layer;

use crate::Ferese;
use crate::metrics::FrameEffectMetrics;
use crate::presentation::{NativeTextureElement, PhysicalShaderElement, physical_rect};

type SurfaceRenderElement =
    CropRenderElement<RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>>;

type WindowRenderElement = CropRenderElement<RelocateRenderElement<RescaleRenderElement<WindowContentRenderElement>>>;

type MemoryRenderElement =
    CropRenderElement<RelocateRenderElement<RescaleRenderElement<MemoryRenderBufferRenderElement<GlesRenderer>>>>;

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

render_elements! {
    WindowContentRenderElement<=GlesRenderer>;
    Rounded=RoundedSurfaceRenderElement,
    Popup=WaylandSurfaceRenderElement<GlesRenderer>,
}

const ROUNDED_TEXTURE_SHADER: &str = include_str!("shaders/rounded_texture_shader.frag");

const ROUNDED_BORDER_SHADER: &str = include_str!("shaders/rounded_border_shader.frag");

const WINDOW_SHADOW_SHADER: &str = include_str!("shaders/window_shadow_shader.frag");

// The material program paints either the fallback tint, the edges above client
// content, or the shadow outside the surface. All coordinates are physical px.
const MATERIAL_SHADER: &str = include_str!("shaders/material_shader.frag");

const BLUR_SHADER: &str = include_str!("shaders/blur_shader.frag");

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

    fn damage_since(&self, scale: RenderScale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
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

    fn damage_since(&self, scale: RenderScale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
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
            WaylandSurfaceTexture::SolidColor(_) => self.inner.draw(frame, src, dst, damage, opaque_regions),
        }
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        self.inner.underlying_storage(renderer)
    }
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

#[cfg(test)]
mod tests {
    use smithay::backend::renderer::damage::OutputDamageTracker;
    use smithay::backend::renderer::element::{Element, Id};
    use smithay::backend::renderer::utils::CommitCounter;
    use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Transform};

    use super::{
        border_gradient_line, color_with_alpha, framebuffer_clip_rect, resize_content_behavior, rounded_visual_rect,
        scaled_visual_rect, shadow_bounds,
    };

    #[test]
    #[ignore = "requires an EGL rendering device"]
    fn translucent_blur_does_not_leak_the_sharp_backdrop() {
        use smithay::backend::allocator::Fourcc;
        use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
        use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture, Uniform};
        use smithay::backend::renderer::{Bind, Color32F, ExportMem, Frame, ImportMem, Offscreen, Renderer};

        let device = EGLDevice::enumerate().unwrap().last().expect("an EGL device");
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
        let blurred = renderer.import_memory(&pixels, Fourcc::Abgr8888, size, false).unwrap();
        let mut target_texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, size).unwrap();
        let damage = Rectangle::<i32, Physical>::from_size((32, 32).into());

        for (opacity, strength, tint) in [
            (0.2f32, 0.5f32, 0.1f32),
            (0.71, 0.5, 0.1),
            (1.0, 0.5, 0.1),
            (0.94, 1.0, 245.0 / 255.0),
        ] {
            let uniforms = vec![
                Uniform::new("visible_rect", [0.0f32, 0.0, 32.0, 32.0]),
                Uniform::new("material_radius", 0.0f32),
                Uniform::new("texture_size", [32.0f32, 32.0]),
                Uniform::new("capture_origin", [0.0f32, 0.0]),
                Uniform::new("blur_radius", 12.0f32),
                Uniform::new("presentation_alpha", 1.0f32),
                Uniform::new("background_opacity", opacity),
                Uniform::new("tint", [tint, tint, tint, strength]),
            ];
            let mut target = renderer.bind(&mut target_texture).unwrap();
            {
                let mut frame = renderer
                    .render(&mut target, (32, 32).into(), Transform::Normal)
                    .unwrap();
                frame.clear(Color32F::new(0.0, 0.0, 0.0, 1.0), &[damage]).unwrap();
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
            let expected = (128.0 * (1.0 - strength * opacity) + tint * 255.0 * strength * opacity).round() as u8;
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
            let rect: Rectangle<i32, Physical> = Rectangle::new((110, 60).into(), (800, 600).into());
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
                assert!(progress(first.into()).abs() < 0.00001, "{transform:?}, {angle}");
                assert!((progress(last.into()) - 1.0).abs() < 0.00001, "{transform:?}, {angle}");
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
            Rectangle::from_size(self.geometry.size.to_f64().to_buffer(1.0, Transform::Normal))
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
        use smithay::backend::renderer::element::utils::{ConstrainAlign, constrain_render_elements};
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
                let output_size =
                    smithay::utils::Size::<i32, Physical>::from(((1280.0 * scale) as i32, (800.0 * scale) as i32));
                let element_size = transform.transform_size(output_size);
                let logical_height = f64::from(element_size.h) / scale;
                // Bar and exclusion end at 40; the 10px outer gap ends at 50.
                let geometry =
                    Rectangle::<f64, Logical>::new((10.0, 50.0).into(), (400.0, logical_height - 60.0).into())
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
                for (actual, expected) in clip.into_iter().zip([min_x, min_y, max_x - min_x, max_y - min_y]) {
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
        let mut tracker = OutputDamageTracker::new((1_000, 800), Scale::from(1.0), Transform::Normal);

        tracker.damage_output(0, &[&shadow]).unwrap();
        shadow.geometry = shadow_bounds(new_window, 4.0, 18.0);
        let damage = tracker.damage_output(1, &[&shadow]).unwrap().0.cloned().unwrap();

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
        assert_eq!(super::material_blur_radius(MaterialStyle::Translucent, 0.0, 18.0), 0.0);
        assert_eq!(super::material_blur_radius(MaterialStyle::Translucent, 1.0, 18.0), 18.0);
        assert_eq!(
            super::material_blur_radius(MaterialStyle::Translucent, 0.79, 18.0),
            18.0
        );
        assert_eq!(super::material_blur_radius(MaterialStyle::Solid, 0.79, 18.0), 0.0);
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
        assert_eq!(color_with_alpha([0.2, 0.4, 0.6, 0.8], 0.5), [0.2, 0.4, 0.6, 0.4]);
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
            let frame = rounded_visual_rect(ferese_layout::Rect::new(left, 50.0, 500.0 - left, 300.0), (0, 0).into());

            assert_eq!(frame.loc.x + frame.size.w, 500);
        }
    }
}
