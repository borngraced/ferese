use std::{fs, sync::OnceLock};

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorIcon,
    utils::{Buffer, Point, Size, Transform},
};
use xcursor::{CursorTheme, parser::Image};

const DEFAULT_CURSOR_SIZE: u32 = 24;

#[derive(Debug, Clone)]
pub(crate) struct NamedCursor {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: Point<i32, Buffer>,
}

pub(crate) fn cursor_theme() -> CursorTheme {
    let name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());

    CursorTheme::load(&name)
}

pub(crate) fn load_named_cursor(theme: &CursorTheme, icon: CursorIcon) -> NamedCursor {
    let requested_size = cursor_size();
    let image = cursor_names(icon)
        .iter()
        .find_map(|name| load_image(theme, name, requested_size));

    match image {
        Some(image) => cursor_from_image(image),
        None => fallback_cursor(),
    }
}

fn cursor_size() -> u32 {
    static SIZE: OnceLock<u32> = OnceLock::new();

    *SIZE.get_or_init(|| {
        std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|size| *size > 0)
            .unwrap_or(DEFAULT_CURSOR_SIZE)
    })
}

fn cursor_names(icon: CursorIcon) -> [&'static str; 3] {
    match icon {
        CursorIcon::Default => ["default", "left_ptr", "arrow"],
        CursorIcon::Pointer => ["pointer", "hand2", "left_ptr"],
        CursorIcon::Text => ["text", "xterm", "left_ptr"],
        _ => [icon.name(), "default", "left_ptr"],
    }
}

fn load_image(theme: &CursorTheme, name: &str, requested_size: u32) -> Option<Image> {
    let path = theme.load_icon(name)?;
    let bytes = fs::read(path).ok()?;

    xcursor::parser::parse_xcursor(&bytes)?
        .into_iter()
        .min_by_key(|image| {
            (
                image.size.abs_diff(requested_size),
                image.width.abs_diff(requested_size),
            )
        })
}

fn cursor_from_image(image: Image) -> NamedCursor {
    let size = Size::<i32, Buffer>::from((image.width as i32, image.height as i32));
    let buffer = MemoryRenderBuffer::from_slice(
        &image.pixels_rgba,
        Fourcc::Abgr8888,
        size,
        1,
        Transform::Normal,
        None,
    );

    NamedCursor {
        buffer,
        hotspot: Point::from((image.xhot as i32, image.yhot as i32)),
    }
}

fn fallback_cursor() -> NamedCursor {
    const WIDTH: usize = 12;
    const HEIGHT: usize = 18;
    let mut pixels = vec![0_u8; WIDTH * HEIGHT * 4];

    for y in 0..HEIGHT {
        let body_width = (y / 2 + 1).min(8);

        for x in 0..body_width {
            let edge = x == 0 || x + 1 == body_width || y == 0;
            let offset = (y * WIDTH + x) * 4;
            let color = if edge { 0 } else { 255 };
            pixels[offset..offset + 4].copy_from_slice(&[color, color, color, 255]);
        }
    }

    NamedCursor {
        buffer: MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Abgr8888,
            Size::<i32, Buffer>::from((WIDTH as i32, HEIGHT as i32)),
            1,
            Transform::Normal,
            None,
        ),
        hotspot: Point::default(),
    }
}

#[cfg(test)]
mod tests {
    use smithay::input::pointer::CursorIcon;

    use super::cursor_names;

    #[test]
    fn supplies_legacy_fallback_names_for_common_cursors() {
        assert_eq!(
            cursor_names(CursorIcon::Default),
            ["default", "left_ptr", "arrow"]
        );
        assert_eq!(
            cursor_names(CursorIcon::Pointer),
            ["pointer", "hand2", "left_ptr"]
        );
        assert_eq!(
            cursor_names(CursorIcon::Text),
            ["text", "xterm", "left_ptr"]
        );
    }

    #[test]
    fn falls_back_to_default_for_other_named_cursors() {
        assert_eq!(
            cursor_names(CursorIcon::Crosshair),
            ["crosshair", "default", "left_ptr"]
        );
    }
}
