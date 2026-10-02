use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::gles::{GlesError, GlesRenderer, GlesTarget, GlesTexture};
use smithay::backend::renderer::{Bind, Offscreen, Texture};
use smithay::output::Output;

use crate::Ferese;
use crate::render::{redraw_output, sampled_output_elements};
use crate::state::FrameScene;

pub(super) fn capture_output(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    texture: &mut Option<GlesTexture>,
    output: &Output,
    scene: &FrameScene,
) -> Result<(), GlesError> {
    for include_cursor in [true, false] {
        if !state.has_pending_screencopy(output, include_cursor) {
            continue;
        }

        // Sample the same scene into an independent target: the displayed
        // primary buffer may omit both a scanned-out client and hardware cursor.
        let elements = sampled_output_elements(state, renderer, output, include_cursor, scene);
        let target = render_capture(renderer, texture, output, &elements)?;
        state.process_screencopies(renderer, &target, output, include_cursor);
    }

    Ok(())
}

pub(super) fn render_capture<'a, E: RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    texture: &'a mut Option<GlesTexture>,
    output: &Output,
    elements: &[E],
) -> Result<GlesTarget<'a>, GlesError> {
    let size = output.current_mode().expect("output has a mode").size;

    if texture
        .as_ref()
        .is_none_or(|texture| texture.size() != (size.w, size.h).into())
    {
        *texture = Some(Offscreen::<GlesTexture>::create_buffer(
            renderer,
            Fourcc::Abgr8888,
            (size.w, size.h).into(),
        )?);
    }

    // Rebinding for each capture restores GL state after readback.
    let mut target = renderer.bind(texture.as_mut().expect("capture target allocated"))?;
    redraw_output(renderer, &mut target, output, elements)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
    use smithay::backend::renderer::element::solid::SolidColorRenderElement;
    use smithay::backend::renderer::element::{Id, Kind};
    use smithay::backend::renderer::utils::CommitCounter;
    use smithay::backend::renderer::{Color32F, ExportMem};
    use smithay::output::{Mode, PhysicalProperties, Subpixel};
    use smithay::utils::{Physical, Rectangle, Transform};

    use super::*;

    #[test]
    #[ignore = "requires an EGL rendering device"]
    fn capture_recomposes_cursor_without_touching_the_display_target() {
        let device = EGLDevice::enumerate().unwrap().last().expect("an EGL device");
        let display = unsafe { EGLDisplay::new(device).unwrap() };
        let context = EGLContext::new(&display).unwrap();
        let mut renderer = unsafe { GlesRenderer::new(context).unwrap() };
        let output = Output::new(
            "capture".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (32, 24).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            None,
            None,
        );
        let background = SolidColorRenderElement::new(
            Id::new(),
            Rectangle::<i32, Physical>::from_size((32, 24).into()),
            CommitCounter::default(),
            Color32F::new(1.0, 0.0, 0.0, 1.0),
            Kind::Unspecified,
        );
        let cursor = SolidColorRenderElement::new(
            Id::new(),
            Rectangle::new((0, 0).into(), (4, 4).into()),
            CommitCounter::default(),
            Color32F::new(0.0, 1.0, 0.0, 1.0),
            Kind::Cursor,
        );
        let mut displayed = None;
        let mut capture = None;
        {
            let _ = render_capture(
                &mut renderer,
                &mut displayed,
                &output,
                std::slice::from_ref(&background),
            )
            .unwrap();
        }

        for (elements, expected) in [
            (vec![cursor, background.clone()], [0, 255, 0, 255]),
            (vec![background], [255, 0, 0, 255]),
        ] {
            let target = render_capture(&mut renderer, &mut capture, &output, &elements).unwrap();
            let mapping = renderer
                .copy_framebuffer(&target, Rectangle::from_size((32, 24).into()), Fourcc::Abgr8888)
                .unwrap();
            let pixels = renderer.map_texture(&mapping).unwrap();
            assert_eq!(&pixels[..4], &expected);
            assert_eq!(&pixels[(12 * 32 + 16) * 4..(12 * 32 + 16) * 4 + 4], &[255, 0, 0, 255]);
            drop(target);

            let target = renderer.bind(displayed.as_mut().unwrap()).unwrap();
            let mapping = renderer
                .copy_framebuffer(&target, Rectangle::from_size((32, 24).into()), Fourcc::Abgr8888)
                .unwrap();
            assert_eq!(&renderer.map_texture(&mapping).unwrap()[..4], &[255, 0, 0, 255]);
        }

        output.change_current_state(
            Some(Mode {
                size: (48, 16).into(),
                refresh: 60_000,
            }),
            Some(Transform::_90),
            None,
            None,
        );
        let _ = render_capture::<SolidColorRenderElement>(&mut renderer, &mut capture, &output, &[]).unwrap();
        assert_eq!(capture.as_ref().unwrap().size(), (48, 16).into());
    }
}
