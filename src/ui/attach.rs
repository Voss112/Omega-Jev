//! Images du chat : presse-papiers, glisser-déposer, redimensionnement et vignettes.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use eframe::egui::{ColorImage, Context, Event, Key, TextureHandle, TextureOptions, Ui, vec2};
use image::{DynamicImage, ImageFormat};

use crate::domain::chat::{Conversation, Image};

/// Beyond this the payload explodes and providers reject the request.
pub const MAX_IMAGES: usize = 4;
const MAX_SIDE: u32 = 1568;
const MAX_BYTES: usize = 2_500_000;
/// Refuse to load a dropped/picked file bigger than this before decoding it.
const MAX_INPUT: u64 = 25_000_000;
/// Decoder caps: a 20 KB PNG can declare a 100 000 × 100 000 canvas and exhaust memory.
const MAX_SIDE_INPUT: u32 = 12_000;
const MAX_ALLOC: u64 = 256 << 20;

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
    encode(downscale(decode(bytes)?))
}

/// Bounded decode: strict width/height caps plus an allocation budget.
fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("image illisible : {e}"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE_INPUT);
    limits.max_image_height = Some(MAX_SIDE_INPUT);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    reader.decode().map_err(|e| format!("image illisible : {e}"))
}

/// Images pasted with Ctrl+V this frame.
pub fn pasted(ctx: &Context) -> Vec<Result<Image, String>> {
    let (shots, files) = ctx.input(|i| {
        let shots: Vec<_> = i
            .events
            .iter()
            .filter_map(|e| match e {
                Event::PasteImage(img) => Some(from_color(img)),
                _ => None,
            })
            .collect();
        // egui swallows Ctrl+V press; its release still arrives. Explorer-copied files carry no text nor bitmap.
        let files = shots.is_empty() && i.modifiers.command && i.key_released(Key::V);
        (shots, files)
    });
    if files { clipboard_files() } else { shots }
}

/// Clipboard bitmap (Win+Shift+S, Snipping Tool, browser "copy image") already decoded by egui.
fn from_color(img: &ColorImage) -> Result<Image, String> {
    let [w, h] = img.size;
    let rgba = img.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
    let buf = image::RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or("image du presse-papiers invalide")?;
    encode(downscale(DynamicImage::ImageRgba8(buf)))
}

/// Image files copied in Explorer (Ctrl+C on a .png).
fn clipboard_files() -> Vec<Result<Image, String>> {
    let Ok(mut clipboard) = arboard::Clipboard::new() else { return Vec::new() };
    clipboard
        .get()
        .file_list()
        .unwrap_or_default()
        .iter()
        .filter(|p| is_image(p))
        .map(|p| from_path(p))
        .collect()
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
        return Ok(Image::new("image/jpeg", STANDARD.encode(&buf)));
    }
    Ok(Image::new("image/png", STANDARD.encode(&buf)))
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

pub fn is_image(path: &Path) -> bool {
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
    let decoded = decode(&bytes).ok()?;
    let rgba = decoded.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = eframe::egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    let handle = ctx.load_texture(format!("shot-{key}"), color, TextureOptions::LINEAR);
    cache.insert(key, handle.clone());
    Some(handle)
}

pub fn key(img: &Image) -> u64 {
    img.key()
}

/// Every image the current view can show: textures outside this set can be freed.
pub fn image_keys(conv: &Conversation, shots: &[Image]) -> HashSet<u64> {
    conv.messages.iter().flat_map(|m| &m.images).chain(shots).map(key).collect()
}

/// One thumbnail; returns `true` when its ✕ was clicked.
pub fn thumb(
    ui: &mut Ui,
    ctx: &Context,
    cache: &mut HashMap<u64, TextureHandle>,
    img: &Image,
    size: f32,
    removable: bool,
) -> bool {
    let mut remove = false;
    if let Some(tex) = texture(ctx, cache, img) {
        let aspect = tex.size_vec2().y / tex.size_vec2().x.max(1.0);
        let resp = ui.add(eframe::egui::Image::new(&tex).fit_to_exact_size(vec2(size, size * aspect)).corner_radius(8.0));
        ui.painter().rect_stroke(
            resp.rect,
            8.0,
            eframe::egui::Stroke::new(1.0, super::theme::EDGE_HI),
            eframe::egui::StrokeKind::Inside,
        );
        if removable {
            let button = eframe::egui::Rect::from_center_size(resp.rect.right_top() + vec2(-11.0, 11.0), vec2(20.0, 20.0));
            let click = ui.interact(button, resp.id.with("remove"), eframe::egui::Sense::click()).on_hover_text("Retirer");
            let p = ui.painter();
            p.circle_filled(
                button.center(),
                10.0,
                eframe::egui::Color32::from_black_alpha(if click.hovered() { 230 } else { 170 }),
            );
            super::icons::at(p, button.center(), 11.0, super::icons::Icon::Close, eframe::egui::Color32::WHITE);
            remove = click.clicked();
        }
    } else {
        ui.label(eframe::egui::RichText::new("image illisible").size(10.5).color(super::theme::RED));
    }
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
    fn an_oversized_canvas_is_refused_before_decoding() {
        assert!(from_bytes(&png(MAX_SIDE_INPUT + 1, 1)).is_err(), "largeur au-delà de la limite");
        assert!(from_bytes(&png(1, MAX_SIDE_INPUT + 1)).is_err(), "hauteur au-delà de la limite");
    }

    #[test]
    fn keys_differ_per_payload() {
        let a = from_bytes(&png(2, 2)).unwrap();
        let b = from_bytes(&png(3, 2)).unwrap();
        assert_ne!(key(&a), key(&b));
    }

    #[test]
    fn image_keys_cover_the_conversation_and_the_composer() {
        let (a, b) = (from_bytes(&png(2, 2)).unwrap(), from_bytes(&png(3, 2)).unwrap());
        let mut conv = crate::domain::chat::Conversation::default();
        conv.push(crate::domain::chat::Message::user_with("x".into(), vec![a.clone()]));
        let keys = image_keys(&conv, std::slice::from_ref(&b));
        assert_eq!(keys, HashSet::from([key(&a), key(&b)]));
    }
}
