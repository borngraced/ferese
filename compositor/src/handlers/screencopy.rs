use std::sync::{Arc, Mutex};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{ExportMem, TextureMapping},
    },
    output::Output,
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            protocol::{wl_buffer::WlBuffer, wl_shm},
        },
    },
    utils::{Buffer, Logical, Rectangle, Size},
    wayland::shm::with_buffer_contents_mut,
};

use crate::Ferese;

const BYTES_PER_PIXEL: usize = 4;

#[derive(Debug)]
pub(crate) struct FrameData {
    output: Option<Output>,
    region: Rectangle<i32, Buffer>,
    used: Mutex<bool>,
}

#[derive(Debug)]
pub(crate) struct PendingScreencopy {
    pub(crate) frame: ZwlrScreencopyFrameV1,
    buffer: WlBuffer,
    pub(crate) output: Output,
    region: Rectangle<i32, Buffer>,
    with_damage: bool,
}

pub(crate) fn init_global(display: &DisplayHandle) {
    let enabled = std::env::var_os("FERESE_ENABLE_SCREENCOPY").is_some_and(|value| value == "1");

    if enabled {
        display.create_global::<Ferese, ZwlrScreencopyManagerV1, ()>(3, ());
        tracing::info!("authorized screencopy is enabled for this session");
    }
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for Ferese {
    fn bind(
        _state: &mut Self,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Ferese {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor: _,
                output,
            } => {
                create_frame(data_init, frame, output, None);
            }
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor: _,
                output,
                x,
                y,
                width,
                height,
            } => {
                create_frame(
                    data_init,
                    frame,
                    output,
                    Some(Rectangle::new((x, y).into(), (width, height).into())),
                );
            }
            zwlr_screencopy_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, Arc<FrameData>> for Ferese {
    fn request(
        state: &mut Self,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &Arc<FrameData>,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => {
                state.queue_screencopy(frame, data, buffer, false);
            }
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => {
                state.queue_screencopy(frame, data, buffer, true);
            }
            zwlr_screencopy_frame_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl Ferese {
    fn queue_screencopy(
        &mut self,
        frame: &ZwlrScreencopyFrameV1,
        data: &Arc<FrameData>,
        buffer: WlBuffer,
        with_damage: bool,
    ) {
        let mut used = data.used.lock().unwrap();
        if *used {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                "screencopy frame has already been used",
            );
            return;
        }
        *used = true;

        if !valid_shm_buffer(&buffer, data.region.size) {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "screencopy requires the advertised ARGB8888 SHM buffer",
            );
            return;
        }

        let Some(output) = data.output.clone() else {
            frame.failed();
            return;
        };

        self.pending_screencopies.push(PendingScreencopy {
            frame: frame.clone(),
            buffer,
            output,
            region: data.region,
            with_damage,
        });
        crate::backends::direct::render_all(self);
    }

    pub(crate) fn process_screencopies<R>(
        &mut self,
        renderer: &mut R,
        framebuffer: &R::Framebuffer<'_>,
        output: &Output,
    ) -> bool
    where
        R: ExportMem,
    {
        let mut remaining = Vec::new();
        let mut framebuffer_binding_changed = false;
        let captures = std::mem::take(&mut self.pending_screencopies);

        for capture in captures {
            if &capture.output != output {
                remaining.push(capture);
                continue;
            }
            if !capture.frame.is_alive() || !capture.buffer.is_alive() {
                continue;
            }

            framebuffer_binding_changed = true;
            if complete_capture(renderer, framebuffer, &capture, self.start_time.elapsed()).is_err()
            {
                capture.frame.failed();
            }
        }

        self.pending_screencopies = remaining;
        framebuffer_binding_changed
    }
}

fn create_frame(
    data_init: &mut DataInit<'_, Ferese>,
    frame: New<ZwlrScreencopyFrameV1>,
    wl_output: smithay::reexports::wayland_server::protocol::wl_output::WlOutput,
    requested: Option<Rectangle<i32, Logical>>,
) {
    let Some(output) = Output::from_resource(&wl_output) else {
        let frame = data_init.init(
            frame,
            Arc::new(FrameData {
                output: None,
                region: Rectangle::from_size((1, 1).into()),
                used: Mutex::new(true),
            }),
        );
        frame.failed();
        return;
    };
    let Some(region) = capture_region(&output, requested) else {
        let frame = data_init.init(
            frame,
            Arc::new(FrameData {
                output: Some(output),
                region: Rectangle::from_size((1, 1).into()),
                used: Mutex::new(true),
            }),
        );
        frame.failed();
        return;
    };

    let size = region.size;
    let data = Arc::new(FrameData {
        output: Some(output),
        region,
        used: Mutex::new(false),
    });
    let frame = data_init.init(frame, data);

    frame.buffer(
        wl_shm::Format::Argb8888,
        size.w as u32,
        size.h as u32,
        (size.w as usize * BYTES_PER_PIXEL) as u32,
    );
    if frame.version() >= 3 {
        frame.buffer_done();
    }
}

fn capture_region(
    output: &Output,
    requested: Option<Rectangle<i32, Logical>>,
) -> Option<Rectangle<i32, Buffer>> {
    let mode = output.current_mode()?;
    let scale = output.current_scale().fractional_scale();
    let transform = output.current_transform();
    capture_region_for_geometry(mode.size, scale, transform, requested)
}

fn capture_region_for_geometry(
    mode_size: Size<i32, smithay::utils::Physical>,
    scale: f64,
    transform: smithay::utils::Transform,
    requested: Option<Rectangle<i32, Logical>>,
) -> Option<Rectangle<i32, Buffer>> {
    let logical_size = transform
        .transform_size(mode_size)
        .to_f64()
        .to_logical(scale)
        .to_i32_round();
    let output_region = Rectangle::from_size(logical_size);
    let logical_region = requested
        .unwrap_or(output_region)
        .intersection(output_region)?;

    if logical_region.size.w <= 0 || logical_region.size.h <= 0 {
        return None;
    }

    Some(
        logical_region
            .to_f64()
            .to_buffer(scale, transform, &logical_size.to_f64())
            .to_i32_round(),
    )
}

fn valid_shm_buffer(buffer: &WlBuffer, expected: Size<i32, Buffer>) -> bool {
    with_buffer_contents_mut(buffer, |_, length, data| {
        let stride = expected.w.checked_mul(BYTES_PER_PIXEL as i32);
        let required = data
            .stride
            .checked_mul(data.height)
            .and_then(|bytes| data.offset.checked_add(bytes));

        data.format == wl_shm::Format::Argb8888
            && data.width == expected.w
            && data.height == expected.h
            && Some(data.stride) == stride
            && required.is_some_and(|required| required >= 0 && required as usize <= length)
    })
    .unwrap_or(false)
}

fn complete_capture<R>(
    renderer: &mut R,
    framebuffer: &R::Framebuffer<'_>,
    capture: &PendingScreencopy,
    timestamp: std::time::Duration,
) -> Result<(), ()>
where
    R: ExportMem,
{
    let mapping = renderer
        .copy_framebuffer(framebuffer, capture.region, Fourcc::Argb8888)
        .map_err(|error| {
            tracing::warn!(?error, "failed to copy output framebuffer");
        })?;
    let source = renderer.map_texture(&mapping).map_err(|error| {
        tracing::warn!(?error, "failed to map output framebuffer copy");
    })?;
    let expected = capture.region.size;
    let copied = with_buffer_contents_mut(&capture.buffer, |destination, length, data| {
        let bytes = expected.w as usize * expected.h as usize * BYTES_PER_PIXEL;
        if source.len() < bytes || data.offset < 0 || data.offset as usize + bytes > length {
            return false;
        }

        // Preserve the renderer's row order and report it through the protocol
        // instead of making another full-frame copy on the compositor thread.
        unsafe {
            std::ptr::copy_nonoverlapping(
                source.as_ptr(),
                destination.add(data.offset as usize),
                bytes,
            );
        }
        true
    })
    .map_err(|_| ())?;

    if !copied {
        return Err(());
    }

    if capture.with_damage {
        capture
            .frame
            .damage(0, 0, expected.w as u32, expected.h as u32);
    }
    let flags = if mapping.flipped() {
        zwlr_screencopy_frame_v1::Flags::YInvert
    } else {
        zwlr_screencopy_frame_v1::Flags::empty()
    };
    capture.frame.flags(flags);

    let seconds = timestamp.as_secs();
    capture.frame.ready(
        (seconds >> 32) as u32,
        seconds as u32,
        timestamp.subsec_nanos(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::{Physical, Transform};

    #[test]
    fn capture_region_clips_then_scales_to_buffer_coordinates() {
        let mode = Size::<i32, Physical>::from((1_920, 1_080));
        let requested = Rectangle::new((-10, 20).into(), (110, 50).into());

        let region =
            capture_region_for_geometry(mode, 2.0, Transform::Normal, Some(requested)).unwrap();

        assert_eq!(region, Rectangle::new((0, 40).into(), (200, 100).into()));
    }

    #[test]
    fn capture_region_rejects_regions_outside_the_output() {
        let mode = Size::<i32, Physical>::from((1_920, 1_080));
        let requested = Rectangle::new((1_000, 600).into(), (100, 100).into());

        assert!(
            capture_region_for_geometry(mode, 2.0, Transform::Normal, Some(requested),).is_none()
        );
    }
}
