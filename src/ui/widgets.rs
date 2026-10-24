//! Reusable visual atoms: glass tiles, gradient buttons, icon buttons, pills, chips, switches, status dots.

use std::sync::Arc;

use eframe::egui::{
    Align2, Color32, CornerRadius, FontId, Frame, Galley, InnerResponse, Margin, Painter, Popup, PopupCloseBehavior, Pos2, Rect,
    Response, RichText, Sense, Shadow, Shape, Stroke, StrokeKind, Ui,
    epaint::{CircleShape, ColorMode, PathStroke},
    text::{LayoutJob, TextWrapping},
    vec2,
};

use super::{
    fx,
    i18n::tr,
    icons::{self, Icon},
    theme::{self, ACCENT, ACCENT_HI, CYAN, EDGE, EDGE_HI, FAINT, INDIGO, MUTED, SOFT, TEXT, TILE, TILE_ACTIVE, TILE_HOVER},
};

/// Single-line galley elided with `…` beyond `max_width`.
pub fn line(ui: &Ui, text: impl Into<String>, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.into(), font, color);
    job.wrap = TextWrapping { max_width: max_width.max(8.0), max_rows: 1, break_anywhere: true, ..Default::default() };
    ui.painter().layout_job(job)
}

/// Glowing status dot that breathes: green = loaded and working, red = inactive.
pub fn status(p: &Painter, c: Pos2, color: Color32, t: f64) {
    let k = fx::pulse(t + f64::from(c.y) * 0.004, 2.6);
    p.circle_filled(c, 5.0 + 2.0 * k, fx::additive(color.gamma_multiply(0.10 + 0.12 * k)));
    p.circle_filled(c, 3.4, color);
    p.circle_filled(c + vec2(-0.9, -0.9), 1.1, Color32::from_white_alpha(70));
}

/// Tinted pill label.
pub fn chip(ui: &mut Ui, text: impl Into<String>, color: Color32) -> Response {
    chip_icon(ui, None, text, color)
}

pub fn chip_icon(ui: &mut Ui, icon: Option<Icon>, text: impl Into<String>, color: Color32) -> Response {
    let text = text.into();
    let galley = line(ui, text.clone(), FontId::proportional(11.5), color, 360.0);
    let icon_w = if icon.is_some() { 15.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_w + 18.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect(rect, 11.0, color.gamma_multiply(0.11), Stroke::new(1.0, color.gamma_multiply(0.32)), StrokeKind::Inside);
    if let Some(icon) = icon {
        icons::at(p, rect.left_center() + vec2(15.0, 0.0), 12.0, icon, color);
    }
    let elided = galley.elided;
    p.galley(rect.left_center() + vec2(9.0 + icon_w, -galley.size().y / 2.0), galley, color);
    if elided { resp.on_hover_text(text) } else { resp }
}

/// Glass tile inside a panel.
pub fn card<R>(ui: &mut Ui, stroke: Color32, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    Frame::new()
        .fill(TILE)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(12))
        .show(ui, add)
}

/// Gradient call-to-action with a glow that swells on hover.
pub fn primary_icon(ui: &mut Ui, icon: Option<Icon>, text: &str, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 38.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.18);
    let rect = if resp.is_pointer_button_down_on() { rect.shrink(1.0) } else { rect };
    let p = ui.painter();
    fx::glow(p, rect, 12.0, ACCENT.gamma_multiply(0.30 + 0.30 * hover), (16.0 + 14.0 * hover) as u8);
    fx::gradient_rect(p, rect, 12.0, fx::mix(ACCENT, ACCENT_HI, 0.35 * hover), fx::mix(INDIGO, CYAN, 0.1 + 0.3 * hover));
    let top = rect.top();
    let h = rect.height();
    fx::gradient_border(p, rect, 12.0, 1.0, move |pos| {
        Color32::from_white_alpha((70.0 - 55.0 * ((pos.y - top) / h).clamp(0.0, 1.0)) as u8)
    });
    let galley = line(ui, text, theme::bold(13.5), Color32::WHITE, width - 40.0);
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let start = rect.center().x - (galley.size().x + icon_w) / 2.0;
    if let Some(icon) = icon {
        icons::at(p, Pos2::new(start + 7.0, rect.center().y), 15.0, icon, Color32::WHITE);
    }
    p.galley(Pos2::new(start + icon_w, rect.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
    resp
}

/// Quiet text button.
pub fn ghost(ui: &mut Ui, text: &str) -> Response {
    pill(ui, None, text, MUTED, false)
}

/// Icon + label button, glass on hover, highlighted while `active`.
pub fn pill(ui: &mut Ui, icon: Option<Icon>, text: &str, color: Color32, active: bool) -> Response {
    let galley = if text.is_empty() { None } else { Some(line(ui, text, FontId::proportional(13.0), color, 260.0)) };
    let text_w = galley.as_ref().map_or(0.0, |g| g.size().x);
    let icon_w = if icon.is_some() { 16.0 } else { 0.0 };
    let gap = if icon.is_some() && galley.is_some() { 7.0 } else { 0.0 };
    let pad = if galley.is_some() { 11.0 } else { 7.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(icon_w + gap + text_w + pad * 2.0, 30.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.14);
    let p = ui.painter();
    let fill = if active { TILE_ACTIVE } else { TILE_HOVER.gamma_multiply(hover) };
    p.rect(rect, 9.0, fill, Stroke::new(1.0, if active { EDGE_HI } else { EDGE.gamma_multiply(hover) }), StrokeKind::Inside);
    let fg = if active { TEXT } else { fx::mix(color, TEXT, hover * 0.8) };
    let mut x = rect.left() + pad;
    if let Some(icon) = icon {
        icons::at(p, Pos2::new(x + icon_w / 2.0, rect.center().y), 16.0, icon, if active { ACCENT_HI } else { fg });
        x += icon_w + gap;
    }
    if let Some(g) = galley {
        p.galley(Pos2::new(x, rect.center().y - g.size().y / 2.0), g, fg);
    }
    resp
}

/// Square icon-only button.
pub fn icon_button(ui: &mut Ui, icon: Icon, tip: &str) -> Response {
    icon_button_sized(ui, icon, 30.0, MUTED, false).on_hover_text(tip)
}

pub fn icon_button_sized(ui: &mut Ui, icon: Icon, size: f32, color: Color32, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    let fill = if active { 0.9 } else { hover };
    if fill > 0.0 {
        p.rect_filled(rect, size * 0.3, TILE_HOVER.gamma_multiply(fill));
    }
    let fg = if active { ACCENT_HI } else { fx::mix(color, TEXT, hover) };
    icons::paint(p, rect.shrink(size * 0.22), icon, fg);
    resp
}

/// Dropdown pill: icon, label, chevron; `add` fills the menu.
pub fn dropdown<R>(
    ui: &mut Ui,
    icon: Option<Icon>,
    text: &str,
    max_width: f32,
    add: impl FnOnce(&mut Ui) -> R,
) -> (Response, Option<R>) {
    let galley = line(ui, text, FontId::proportional(12.5), SOFT, max_width);
    let icon_w = if icon.is_some() { 21.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(icon_w + galley.size().x + 44.0, 30.0), Sense::click());
    let open = Popup::is_id_open(ui.ctx(), Popup::default_response_id(&resp));
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || open, 0.14);
    let p = ui.painter();
    p.rect(
        rect,
        10.0,
        fx::mix(TILE, TILE_HOVER, hover),
        Stroke::new(1.0, fx::mix(EDGE, ACCENT.gamma_multiply(0.55), hover)),
        StrokeKind::Inside,
    );
    let fg = fx::mix(SOFT, TEXT, hover);
    if let Some(icon) = icon {
        icons::at(p, rect.left_center() + vec2(18.0, 0.0), 14.0, icon, fx::mix(ACCENT_HI, CYAN, hover));
    }
    p.galley(rect.left_center() + vec2(12.0 + icon_w, -galley.size().y / 2.0), galley, fg);
    let flip = ui.ctx().animate_bool_with_time(resp.id.with("open"), open, 0.18);
    icons::chevron(ui.painter(), rect.right_center() - vec2(15.0, 0.0), 13.0, 1.0 - 2.0 * flip, fx::mix(MUTED, TEXT, hover));
    let inner = Popup::menu(&resp)
        .close_behavior(PopupCloseBehavior::CloseOnClick)
        .gap(6.0)
        .show(|ui| {
            ui.set_min_width(rect.width().max(200.0));
            add(ui)
        })
        .map(|r| r.inner);
    (resp, inner)
}

/// Letter-spaced section heading.
pub fn section(ui: &mut Ui, text: &str) {
    let job = fx::spaced(&text.to_uppercase(), theme::bold(10.0), FAINT, 1.1);
    ui.label(job);
}

/// Ω medallion: gradient disc, slowly turning prismatic rim, soft halo.
pub fn paint_logo(p: &Painter, rect: Rect, t: f64) {
    let (c, r) = (rect.center(), rect.width() / 2.0);
    fx::glow(p, rect.shrink(r * 0.2), r, ACCENT.gamma_multiply(0.30 + 0.12 * fx::pulse(t, 4.0)), (r * 1.1) as u8);
    let disc = CircleShape::filled(c, r - 1.0, Color32::WHITE);
    fx::paint_with(p, disc, fx::linear(rect, ACCENT, INDIGO, vec2(0.8, 1.0)));
    let points: Vec<Pos2> =
        (0..64).map(|i| i as f32 / 64.0 * std::f32::consts::TAU).map(|a| c + vec2(a.cos(), a.sin()) * (r - 1.0)).collect();
    let rim = fx::conic(rect, t, 1.0);
    let stroke =
        PathStroke { width: (r * 0.12).max(1.4), color: ColorMode::UV(Arc::new(move |_, pos| rim(pos))), ..Default::default() };
    p.add(Shape::closed_line(points, stroke));
    p.text(c + vec2(0.0, r * 0.02), Align2::CENTER_CENTER, "Ω", theme::bold(r * 1.12), Color32::WHITE);
}

/// Animated on/off switch with a label; returns the click response.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let galley = line(ui, label, FontId::proportional(13.0), SOFT, ui.available_width() - 50.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(44.0 + galley.size().x, 22.0), Sense::click());
    // A cut description stays readable: the whole text shows in a tooltip.
    let resp = if galley.elided { resp.on_hover_text(label) } else { resp };
    if resp.clicked() {
        *on = !*on;
    }
    let k = ui.ctx().animate_bool_with_time(resp.id, *on, 0.16);
    let track = Rect::from_min_size(rect.min, vec2(36.0, 20.0)).translate(vec2(0.0, 1.0));
    let p = ui.painter();
    if k > 0.0 {
        fx::glow(p, track, 10.0, ACCENT.gamma_multiply(0.35 * k), 14);
    }
    p.rect(track, 10.0, TILE_ACTIVE, Stroke::new(1.0, EDGE_HI), StrokeKind::Inside);
    if k > 0.0 {
        fx::gradient_rect(p, track, 10.0, ACCENT.gamma_multiply(k), CYAN.gamma_multiply(k * 0.85));
    }
    let knob = Pos2::new(track.left() + 10.0 + 16.0 * k, track.center().y);
    p.circle_filled(knob + vec2(0.0, 1.0), 7.5, Color32::from_black_alpha(60));
    p.circle_filled(knob, 7.0, fx::mix(SOFT, Color32::WHITE, k));
    let fg = if resp.hovered() { TEXT } else { SOFT };
    p.galley(Pos2::new(track.right() + 8.0, rect.center().y - galley.size().y / 2.0), galley, fg);
    resp
}

/// Thin rounded progress bar with a gradient fill.
pub fn bar(ui: &mut Ui, value: f32, color: Color32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, height / 2.0, TILE_ACTIVE);
    let value = value.clamp(0.0, 1.0);
    if value > 0.0 {
        let filled = Rect::from_min_size(rect.min, vec2((rect.width() * value).max(height), height));
        fx::gradient_rect(p, filled, height / 2.0, color.gamma_multiply(0.55), color);
    }
}

/// Label truncated to the available width, full text on hover.
pub fn truncated(ui: &mut Ui, text: &str, font: FontId, color: Color32) -> Response {
    let galley = line(ui, text, font, color, ui.available_width());
    let elided = galley.elided;
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, color);
    if elided { resp.on_hover_text(text) } else { resp }
}

/// Menu row with a check mark on the current value.
pub fn menu_item(ui: &mut Ui, selected: bool, text: &str) -> Response {
    let galley = line(ui, text, FontId::proportional(13.0), if selected { TEXT } else { SOFT }, 360.0);
    let width = (galley.size().x + 52.0).max(ui.available_width());
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
    let k = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    let p = ui.painter();
    if selected || k > 0.0 {
        p.rect_filled(rect, 8.0, if selected { ACCENT.gamma_multiply(0.18) } else { TILE_HOVER.gamma_multiply(k) });
    }
    if selected {
        icons::at(p, rect.left_center() + vec2(15.0, 0.0), 14.0, Icon::Check, ACCENT_HI);
    }
    let fg = if selected { TEXT } else { fx::mix(SOFT, TEXT, k) };
    p.galley(rect.left_center() + vec2(32.0, -galley.size().y / 2.0), galley, fg);
    resp
}

/// Dims the window behind a modal.
pub const BACKDROP: Color32 = Color32::from_black_alpha(150);

/// Frosted modal card.
pub fn modal_frame() -> Frame {
    Frame::new()
        .fill(theme::GLASS_STRONG)
        .stroke(Stroke::new(1.0, EDGE_HI))
        .corner_radius(CornerRadius::same(22))
        .inner_margin(Margin::same(26))
        .shadow(Shadow { offset: [0, 24], blur: 64, spread: 0, color: Color32::from_black_alpha(190) })
}

/// Badge, title, subtitle and a close button; returns `true` when closed.
pub fn modal_header(ui: &mut Ui, icon: Icon, title: &str, subtitle: &str) -> bool {
    let mut close = false;
    ui.horizontal_top(|ui| {
        let (badge, _) = ui.allocate_exact_size(vec2(42.0, 42.0), Sense::hover());
        let p = ui.painter();
        fx::glow(p, badge, 13.0, ACCENT.gamma_multiply(0.3), 20);
        fx::gradient_rect(p, badge, 13.0, ACCENT, INDIGO);
        p.add(fx::edge_light(badge, 13.0, 90.0, 12.0));
        icons::paint(p, badge.shrink(11.0), icon, Color32::WHITE);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.set_width(ui.available_width() - 40.0);
            ui.label(RichText::new(title).font(theme::bold(20.0)).color(TEXT));
            ui.label(RichText::new(subtitle).size(12.5).color(MUTED));
        });
        ui.with_layout(eframe::egui::Layout::right_to_left(eframe::egui::Align::Min), |ui| {
            close = icon_button(ui, Icon::Close, tr("Fermer (Échap)", "Close (Esc)")).clicked();
        });
    });
    close
}

/// Uppercase section title with an icon, for forms.
pub fn form_section(ui: &mut Ui, icon: Icon, title: &str) {
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(15.0, 15.0), Sense::hover());
        icons::paint(ui.painter(), rect, icon, ACCENT_HI);
        ui.add_space(2.0);
        ui.label(fx::spaced(&title.to_uppercase(), theme::bold(10.5), SOFT, 1.1));
    });
    ui.add_space(6.0);
}

pub fn human(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

/// Plain `RichText` shortcut.
pub fn text(s: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(s).size(size).color(color)
}
