//! Images du chat : presse-papiers, glisser-déposer, redimensionnement et vignettes.

use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    path::Path,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use eframe::egui::{Context, TextureHandle, TextureOptions, Ui, vec2};
use image::{DynamicImage, ImageFormat};

use crate::domain::chat::Image;

/// Beyond this the payload explodes and providers reject the request.
pub const MAX_IMAGES: usize = 4;
const MAX_SIDE: u32 = 1568;
const MAX_BYTES: usize = 2_500_000;
/// Refuse to load a dropped/picked file bigger than this before decoding it.
const MAX_INPUT: u64 = 25_000_000;

pub fn from_path(path: &Path) -> Result<Image, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("{} : {e}", path.display()))?.len();
    if size > MAX_INPUT {
        return Err(format!("{} : image trop lourde (25 Mo max)", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{} : {e}", path.display()))?;
    from_bytes(&bytes)
}

/// Decodes, downscales to 1568 px and re-encodes; PNG for crisp text, JPEG when PNG stays too heavy.
pub fn from_bytes(bytes: &[u8]) -> Result<Image, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("image illisible : {e}"))?;
    encode(downscale(img))
}

/// Clipboard image (Win+Shift+S screenshot ⇒ Ctrl+V here).
pub fn from_clipboard() -> Option<Image> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let data = clipboard.get_image().ok()?;
    let img = image::RgbaImage::from_raw(data.width as u32, data.height as u32, data.bytes.into_owned())?;
    encode(downscale(DynamicImage::ImageRgba8(img))).ok()
}

/// PNG keeps text crisp; JPEG only when the PNG would blow past the provider limit.
fn encode(img: DynamicImage) -> Result<Image, String> {
    let mut buf = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png).map_err(|e| format!("encodage PNG : {e}"))?;
    if buf.len() > MAX_BYTES {
        buf.clear();
        let mut cursor = std::io::Cursor::new(&mut buf);
        DynamicImage::ImageRgba8(img.to_rgba8())
            .write_to(&mut cursor, ImageFormat::Jpeg)
            .map_err(|e| format!("encodage JPEG : {e}"))?;
        if buf.len() > MAX_BYTES {
            return Err("image trop lourde même après compression".into());
        }
        return Ok(Image { media: "image/jpeg".into(), data: STANDARD.encode(&buf) });
    }
    Ok(Image { media: "image/png".into(), data: STANDARD.encode(&buf) })
}
/// Bytes of image files dropped on the window during this frame.
pub fn dropped(ctx: &Context) -> Vec<Result<Vec<u8>, String>> {
    ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .filter(|f| is_image(f.path()))
            .map(|f| {
                let path = f.path();
                match std::fs::metadata(path) {
                    Ok(m) if m.len() > MAX_INPUT => Err(format!("{} : image trop lourde (25 Mo max)", path.display())),
                    Ok(_) => f.bytes().map_err(|e| format!("{} : {e}", path.display())),
                    Err(e) => Err(format!("{} : {e}", path.display())),
                }
            })
            .collect()
    })
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"))
}

fn downscale(img: DynamicImage) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w.max(h) <= MAX_SIDE {
        return img;
    }
    let scale = MAX_SIDE as f32 / w.max(h) as f32;
    let (nw, nh) = ((w as f32 * scale).round() as u32, (h as f32 * scale).round() as u32);
    img.resize(nw.max(1), nh.max(1), image::imageops::FilterType::Triangle)
}

/// Cached GPU texture for a data URL; decoding happens once per image.
pub fn texture(ctx: &Context, cache: &mut HashMap<u64, TextureHandle>, img: &Image) -> Option<TextureHandle> {
    let key = key(img);
    if let Some(handle) = cache.get(&key) {
        return Some(handle.clone());
    }
    let bytes = STANDARD.decode(&img.data).ok()?;
    let decoded = image::load_from_memory(&bytes).ok()?;
    let rgba = decoded.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = eframe::egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let handle = ctx.load_texture(format!("shot-{key}"), color, TextureOptions::LINEAR);
    cache.insert(key, handle.clone());
    Some(handle)
}

pub fn key(img: &Image) -> u64 {
    let mut hasher = DefaultHasher::new();
    img.media.hash(&mut hasher);
    img.data.hash(&mut hasher);
    hasher.finish()
}

/// One thumbnail; returns `true` when its ✕ was clicked.
pub fn thumb(ui: &mut Ui, ctx: &Context, cache: &mut HashMap<u64, TextureHandle>, img: &Image, size: f32, removable: bool) -> bool {
    let mut remove = false;
    ui.vertical(|ui| {
        if let Some(tex) = texture(ctx, cache, img) {
            let aspect = tex.size_vec2().y / tex.size_vec2().x.max(1.0);
            ui.add(eframe::egui::Image::new(&tex).fit_to_exact_size(vec2(size, size * aspect)).corner_radius(6.0));
        } else {
            ui.label(eframe::egui::RichText::new("image illisible").size(10.5).color(super::theme::RED));
        }
        if removable && ui.small_button("✕").clicked() {
            remove = true;
        }
    });
    remove
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(width, height, |x, _| image::Rgba([(x % 256) as u8, 40, 60, 255]));
        let mut buf = Vec::new();
        DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Png).unwrap();
        buf
    }

    #[test]
    fn a_screenshot_becomes_a_decodable_png() {
        let image = from_bytes(&png(4, 3)).unwrap();
        assert_eq!(image.media, "image/png");
        let raw = STANDARD.decode(&image.data).unwrap();
        let decoded = image::load_from_memory(&raw).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4, 3));
    }

    #[test]
    fn oversized_images_are_downscaled() {
        let image = from_bytes(&png(3000, 1000)).unwrap();
        let decoded = image::load_from_memory(&STANDARD.decode(&image.data).unwrap()).unwrap();
        assert_eq!(decoded.width(), 1568);
        assert!(decoded.height() <= 600);
    }

    #[test]
    fn garbage_is_rejected_instead_of_panicking() {
        assert!(from_bytes(b"not an image").is_err());
    }

    #[test]
    fn keys_differ_per_payload() {
        let a = from_bytes(&png(2, 2)).unwrap();
        let b = from_bytes(&png(3, 2)).unwrap();
        assert_ne!(key(&a), key(&b));
    }
}

