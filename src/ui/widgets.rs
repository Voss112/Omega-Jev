//! Reusable visual atoms.

use eframe::egui::{
    Align2, Button, Color32, CornerRadius, FontId, Frame, InnerResponse, Margin, Response, RichText, Sense, Stroke, Ui, vec2,
};

use super::theme::{self, ACCENT, CARD, CYAN, FAINT, GREEN, MUTED, RED};

/// Glowing status dot: green = loaded and working, red = inactive.
pub fn dot(ui: &mut Ui, ok: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
    let color = if ok { GREEN } else { RED };
    ui.painter().circle_filled(rect.center(), 6.0, color.gamma_multiply(0.2));
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

pub fn chip(ui: &mut Ui, text: impl Into<String>, color: Color32) -> Response {
    Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(99))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| ui.label(RichText::new(text.into()).size(11.5).color(color)))
        .response
}

pub fn card<R>(ui: &mut Ui, stroke: Color32, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(12))
        .show(ui, add)
}

pub fn primary(ui: &mut Ui, text: &str, width: f32) -> Response {
    ui.add(
        Button::new(RichText::new(text).color(Color32::WHITE).font(theme::bold(14.0)))
            .fill(ACCENT)
            .corner_radius(CornerRadius::same(10))
            .min_size(vec2(width, 38.0)),
    )
}

pub fn ghost(ui: &mut Ui, text: &str) -> Response {
    ui.add(Button::new(RichText::new(text).color(MUTED)).frame(false))
}

pub fn section(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(10.5).color(FAINT).strong());
}

/// Ω medallion with a violet→cyan rim.
pub fn logo(ui: &mut Ui, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let p = ui.painter();
    let r = size / 2.0;
    p.circle_filled(rect.center(), r, CYAN.gamma_multiply(0.85));
    p.circle_filled(rect.center() + vec2(-r * 0.12, -r * 0.12), r * 0.9, ACCENT);
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        "Ω",
        FontId::new(size * 0.58, eframe::egui::FontFamily::Proportional),
        Color32::WHITE,
    );
    resp
}

pub fn human(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}
