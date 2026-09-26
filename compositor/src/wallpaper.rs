//! One decoded image and one texture per GPU context, not double-buffered
//! full-screen UI surfaces per monitor. Resize changes sampling only.
use crate::presentation::NativeTextureElement;
use serde::Deserialize;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ErasedContextId, ImportMem, Renderer,
            element::Id,
            gles::{GlesRenderer, GlesTexture},
            utils::CommitCounter,
        },
    },
    output::Output,
    utils::{Buffer, Physical, Rectangle, Size},
};
use std::{collections::HashMap, path::PathBuf, sync::mpsc};

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct WallpaperConfig {
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub mode: WallpaperMode,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WallpaperMode {
    #[default]
    Fill,
    Fit,
}

struct WallpaperTexture {
    texture: GlesTexture,
    id: Id,
}
pub(crate) struct WallpaperState {
    owned: bool,
    mode: WallpaperMode,
    receiver: Option<mpsc::Receiver<Result<image::RgbaImage, String>>>,
    pixels: Option<image::RgbaImage>,
    textures: HashMap<ErasedContextId, WallpaperTexture>,
}

impl WallpaperState {
    pub fn new(config: WallpaperConfig) -> Self {
        let (sender, receiver) = mpsc::channel();
        let owned = config.path.as_ref().is_some_and(|path| path.is_file());
        if owned {
            let path = config.path.unwrap();
            std::thread::spawn(move || {
                let load = || -> Result<image::RgbaImage, String> {
                    let (width, height) =
                        image::image_dimensions(&path).map_err(|e| e.to_string())?;
                    if u64::from(width) * u64::from(height) * 4 > 256 * 1024 * 1024 {
                        return Err("wallpaper exceeds the 256 MiB decode limit".into());
                    }
                    image::open(path)
                        .map(|image| image.into_rgba8())
                        .map_err(|e| e.to_string())
                };
                let _ = sender.send(load());
            });
        }
        Self {
            owned,
            mode: config.mode,
            receiver: owned.then_some(receiver),
            pixels: None,
            textures: HashMap::new(),
        }
    }
    pub fn owns_background(&self) -> bool {
        self.owned
    }
    pub fn forget_context(&mut self, context: &ErasedContextId) {
        self.textures.remove(context);
    }
    pub fn poll(&mut self) -> bool {
        let Some(receiver) = &self.receiver else {
            return false;
        };
        match receiver.try_recv() {
            Ok(result) => {
                self.receiver = None;
                match result {
                    Ok(pixels) => {
                        tracing::info!(
                            width = pixels.width(),
                            height = pixels.height(),
                            bytes = pixels.as_raw().len(),
                            "decoded compositor wallpaper once"
                        );
                        self.pixels = Some(pixels);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "wallpaper unavailable; using output clear color")
                    }
                }
                true
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                false
            }
            Err(mpsc::TryRecvError::Empty) => false,
        }
    }
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &Output,
    ) -> Option<NativeTextureElement> {
        let pixels = self.pixels.as_ref()?;
        let context = renderer.context_id().erased();
        if !self.textures.contains_key(&context) {
            let size = Size::from((pixels.width() as i32, pixels.height() as i32));
            match renderer.import_memory(pixels.as_raw(), Fourcc::Abgr8888, size, false) {
                Ok(texture) => {
                    tracing::debug!(
                        bytes = pixels.as_raw().len(),
                        "uploaded shared wallpaper texture"
                    );
                    self.textures.insert(
                        context.clone(),
                        WallpaperTexture {
                            texture,
                            id: Id::new(),
                        },
                    );
                }
                Err(error) => {
                    tracing::debug!(%error, "wallpaper texture import failed");
                    return None;
                }
            }
        }
        let cached = self.textures.get(&context)?;
        let output_size = output
            .current_transform()
            .transform_size(output.current_mode()?.size);
        let (geometry, source) = image_geometry(
            (pixels.width() as i32, pixels.height() as i32).into(),
            output_size,
            self.mode,
        );
        Some(NativeTextureElement {
            id: cached.id.clone(),
            commit: CommitCounter::default(),
            texture: cached.texture.clone(),
            geometry,
            source,
            alpha: 1.0,
            program: None,
            uniforms: vec![],
        })
    }
}

fn image_geometry(
    image: Size<i32, Buffer>,
    output: Size<i32, Physical>,
    mode: WallpaperMode,
) -> (Rectangle<i32, Physical>, Rectangle<f64, Buffer>) {
    let sx = f64::from(output.w) / f64::from(image.w);
    let sy = f64::from(output.h) / f64::from(image.h);
    match mode {
        WallpaperMode::Fill => {
            let scale = sx.max(sy);
            let width = f64::from(output.w) / scale;
            let height = f64::from(output.h) / scale;
            (
                Rectangle::from_size(output),
                Rectangle::new(
                    (
                        (f64::from(image.w) - width) / 2.0,
                        (f64::from(image.h) - height) / 2.0,
                    )
                        .into(),
                    (width, height).into(),
                ),
            )
        }
        WallpaperMode::Fit => {
            let scale = sx.min(sy);
            let size: Size<i32, Physical> = (
                (f64::from(image.w) * scale).round() as i32,
                (f64::from(image.h) * scale).round() as i32,
            )
                .into();
            (
                Rectangle::new(
                    ((output.w - size.w) / 2, (output.h - size.h) / 2).into(),
                    size,
                ),
                Rectangle::from_size(image.to_f64()),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fill_crops_and_fit_letterboxes_without_reallocating_image() {
        let (geometry, source) = image_geometry(
            (3840, 2160).into(),
            (1600, 1000).into(),
            WallpaperMode::Fill,
        );
        assert_eq!(geometry.size, (1600, 1000).into());
        assert_eq!(source.size.h, 2160.0);
        assert!(source.loc.x > 0.0);
        let (geometry, source) =
            image_geometry((3840, 2160).into(), (1600, 1000).into(), WallpaperMode::Fit);
        assert_eq!(geometry.size, (1600, 900).into());
        assert_eq!(geometry.loc, (0, 50).into());
        assert_eq!(source.size, (3840.0, 2160.0).into());
    }
    #[test]
    fn missing_wallpaper_preserves_external_shell_fallback() {
        let state = WallpaperState::new(WallpaperConfig::default());
        assert!(!state.owns_background());
    }
}
