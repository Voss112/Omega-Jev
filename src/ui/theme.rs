//! Visual identity: deep-night palette, violet→cyan accent, Inter + JetBrains Mono.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke, TextStyle, Theme,
    ThemePreference, Visuals,
};

pub const BG: Color32 = Color32::from_rgb(0x0B, 0x0D, 0x12);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x0F, 0x12, 0x19);
pub const CARD: Color32 = Color32::from_rgb(0x15, 0x19, 0x23);
pub const RAISED: Color32 = Color32::from_rgb(0x1B, 0x20, 0x2C);
pub const HOVER: Color32 = Color32::from_rgb(0x23, 0x29, 0x38);
pub const BORDER: Color32 = Color32::from_rgb(0x27, 0x2D, 0x3C);
pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xE9, 0xF2);
pub const SOFT: Color32 = Color32::from_rgb(0xB4, 0xBA, 0xC8);
pub const MUTED: Color32 = Color32::from_rgb(0x8C, 0x95, 0xAA);
pub const FAINT: Color32 = Color32::from_rgb(0x5A, 0x62, 0x76);
pub const ACCENT: Color32 = Color32::from_rgb(0x8B, 0x5C, 0xF6);
pub const CYAN: Color32 = Color32::from_rgb(0x22, 0xD3, 0xEE);
pub const GREEN: Color32 = Color32::from_rgb(0x34, 0xD3, 0x99);
pub const RED: Color32 = Color32::from_rgb(0xF8, 0x71, 0x71);
pub const AMBER: Color32 = Color32::from_rgb(0xFB, 0xBF, 0x24);
pub const BUBBLE: Color32 = Color32::from_rgb(0x24, 0x20, 0x3E);
pub const CODE_BG: Color32 = Color32::from_rgb(0x10, 0x13, 0x1B);

/// Max width of the reading column.
pub const COLUMN: f32 = 860.0;

const BOLD: &str = "bold";
const INTER: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
const INTER_BOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");
const MONO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf");

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [("inter", INTER), ("inter-bold", INTER_BOLD), ("mono", MONO)] {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "mono".into());
    fonts.families.insert(FontFamily::Name(BOLD.into()), std::iter::once("inter-bold".to_owned()).chain(fallbacks).collect());
    ctx.set_fonts(fonts);

    ctx.options_mut(|o| o.theme_preference = ThemePreference::Dark);
    ctx.set_visuals_of(Theme::Dark, visuals());
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, bold(22.0)),
            (TextStyle::Body, FontId::proportional(14.5)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(11.5)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.interact_size.y = 28.0;
    });
}

fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.extreme_bg_color = RAISED;
    v.faint_bg_color = CARD;
    v.code_bg_color = CODE_BG;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(16);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_shadow = Shadow { offset: [0, 14], blur: 40, spread: 0, color: Color32::from_black_alpha(150) };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(120) };
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = CYAN;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = CARD;
    w.noninteractive.weak_bg_fill = CARD;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.bg_fill = RAISED;
    w.inactive.weak_bg_fill = RAISED;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.bg_fill = HOVER;
    w.hovered.weak_bg_fill = HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
    w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.active.bg_fill = ACCENT;
    w.active.weak_bg_fill = ACCENT.gamma_multiply(0.35);
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.open.weak_bg_fill = HOVER;
    for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = CornerRadius::same(8);
    }
    v
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t) as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// Procedural app icon: gradient squircle with a white Ω.
pub fn icon() -> egui::IconData {
    const S: usize = 64;
    let size = S as f32;
    let mut rgba = Vec::with_capacity(S * S * 4);
    for y in 0..S {
        for x in 0..S {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let (cx, cy) = (fx.clamp(14.0, size - 14.0), fy.clamp(14.0, size - 14.0));
            let inside = (fx - cx).powi(2) + (fy - cy).powi(2) <= 196.0;
            let d = ((fx - 32.0).powi(2) + (fy - 29.0).powi(2)).sqrt();
            let angle = (fy - 29.0).atan2(fx - 32.0).to_degrees();
            let ring = (12.5..18.5).contains(&d) && !(50.0..130.0).contains(&angle);
            let feet = (43.0..48.0).contains(&fy) && ((14.0..27.0).contains(&fx) || (37.0..50.0).contains(&fx));
            let bg = lerp(ACCENT, CYAN, (fx + fy) / (2.0 * size));
            let px = match (inside, ring || feet) {
                (false, _) => [0, 0, 0, 0],
                (true, true) => [255, 255, 255, 255],
                (true, false) => [bg.r(), bg.g(), bg.b(), 255],
            };
            rgba.extend_from_slice(&px);
        }
    }
    egui::IconData { rgba, width: S as u32, height: S as u32 }
}
