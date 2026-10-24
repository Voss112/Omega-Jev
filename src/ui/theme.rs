//! Visual identity: aurora night, frosted glass, violet→cyan accent, Inter + JetBrains Mono.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke, TextStyle, Theme,
    ThemePreference, Visuals, style::ScrollStyle,
};

/// Premultiplied colour from straight RGBA, usable in `const`.
pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    let (a16, half) = (a as u16, 127);
    Color32::from_rgba_premultiplied(
        ((r as u16 * a16 + half) / 255) as u8,
        ((g as u16 * a16 + half) / 255) as u8,
        ((b as u16 * a16 + half) / 255) as u8,
        a,
    )
}

pub const BG: Color32 = Color32::from_rgb(0x06, 0x07, 0x0D);
/// Frosted panels laid over the aurora.
pub const GLASS: Color32 = rgba(16, 18, 32, 168);
/// Modals, menus and tooltips: nearly opaque so text stays crisp over anything.
pub const GLASS_STRONG: Color32 = rgba(15, 17, 29, 244);
/// Tiles inside a glass panel.
pub const TILE: Color32 = rgba(255, 255, 255, 9);
pub const TILE_HOVER: Color32 = rgba(255, 255, 255, 17);
pub const TILE_ACTIVE: Color32 = rgba(255, 255, 255, 24);
pub const EDGE: Color32 = rgba(255, 255, 255, 20);
pub const EDGE_HI: Color32 = rgba(255, 255, 255, 40);
pub const INPUT: Color32 = rgba(3, 4, 10, 120);
pub const CODE_BG: Color32 = rgba(5, 6, 13, 175);

pub const TEXT: Color32 = Color32::from_rgb(0xEE, 0xF0, 0xF8);
pub const BODY: Color32 = Color32::from_rgb(0xD9, 0xDD, 0xEA);
pub const SOFT: Color32 = Color32::from_rgb(0xBA, 0xC0, 0xD2);
pub const MUTED: Color32 = Color32::from_rgb(0x8E, 0x95, 0xAD);
pub const FAINT: Color32 = Color32::from_rgb(0x60, 0x68, 0x83);

pub const ACCENT: Color32 = Color32::from_rgb(0x8B, 0x5C, 0xF6);
pub const ACCENT_HI: Color32 = Color32::from_rgb(0xA7, 0x8B, 0xFA);
pub const INDIGO: Color32 = Color32::from_rgb(0x63, 0x66, 0xF1);
pub const CYAN: Color32 = Color32::from_rgb(0x22, 0xD3, 0xEE);
pub const PINK: Color32 = Color32::from_rgb(0xE8, 0x79, 0xF9);
pub const GREEN: Color32 = Color32::from_rgb(0x34, 0xD3, 0x99);
pub const RED: Color32 = Color32::from_rgb(0xF8, 0x71, 0x71);
pub const AMBER: Color32 = Color32::from_rgb(0xFB, 0xBF, 0x24);

/// Max width of the reading column.
pub const COLUMN: f32 = 860.0;
/// Gap between floating panels and around the window edge.
pub const GAP: i8 = 10;
pub const PANEL_RADIUS: f32 = 18.0;

const BOLD: &str = "bold";
const INTER: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
const INTER_BOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");
const MONO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf");
/// 🗐 and ✔ from emoji-icon-font: the markdown copy button needs them and no Windows font has 🗐.
const COPY_GLYPHS: &[u8] = include_bytes!("../../assets/fonts/copy-glyphs.ttf");

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [("inter", INTER), ("inter-bold", INTER_BOLD), ("mono", MONO), ("copy-glyphs", COPY_GLYPHS)] {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("copy-glyphs".into());
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
            (TextStyle::Button, FontId::proportional(13.5)),
            (TextStyle::Small, FontId::proportional(11.5)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.menu_margin = egui::Margin::same(6);
        style.spacing.combo_height = 320.0;
        style.spacing.scroll = ScrollStyle { bar_width: 8.0, floating_width: 3.0, ..ScrollStyle::floating() };
        style.animation_time = 0.16;
    });
}

fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = GLASS_STRONG;
    v.extreme_bg_color = CODE_BG;
    v.text_edit_bg_color = Some(INPUT);
    v.faint_bg_color = TILE;
    v.code_bg_color = rgba(167, 139, 250, 34);
    v.window_stroke = Stroke::new(1.0, EDGE_HI);
    v.window_corner_radius = CornerRadius::same(18);
    v.menu_corner_radius = CornerRadius::same(12);
    v.window_shadow = Shadow { offset: [0, 18], blur: 48, spread: 0, color: Color32::from_black_alpha(170) };
    v.popup_shadow = Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(150) };
    v.window_highlight_topmost = false;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.42);
    v.selection.stroke = Stroke::new(1.0, TEXT);
    v.hyperlink_color = Color32::from_rgb(0x7D, 0xE3, 0xF4);
    v.text_cursor.stroke = Stroke::new(2.0, ACCENT_HI);
    v.indent_has_left_vline = false;
    v.collapsing_header_frame = false;
    v.striped = false;
    v.interact_cursor = Some(CursorIcon::PointingHand);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = TILE;
    w.noninteractive.weak_bg_fill = TILE;
    w.noninteractive.bg_stroke = Stroke::new(1.0, EDGE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, BODY);
    w.inactive.bg_fill = rgba(255, 255, 255, 16);
    w.inactive.weak_bg_fill = rgba(255, 255, 255, 12);
    w.inactive.bg_stroke = Stroke::new(1.0, EDGE);
    w.inactive.fg_stroke = Stroke::new(1.0, SOFT);
    w.hovered.bg_fill = rgba(255, 255, 255, 26);
    w.hovered.weak_bg_fill = rgba(255, 255, 255, 22);
    w.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.7));
    w.hovered.fg_stroke = Stroke::new(1.5, TEXT);
    w.active.bg_fill = ACCENT;
    w.active.weak_bg_fill = ACCENT.gamma_multiply(0.4);
    w.active.bg_stroke = Stroke::new(1.0, ACCENT_HI);
    w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.open.weak_bg_fill = rgba(255, 255, 255, 22);
    w.open.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
    for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = CornerRadius::same(10);
        state.expansion = 0.0;
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
