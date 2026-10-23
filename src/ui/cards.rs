//! Tool cards, approval cards, code and diff blocks.

use eframe::egui::{
    Color32, CornerRadius, FontId, Frame, Label, Margin, RichText, ScrollArea, Spinner, Stroke, TextFormat, Ui,
    collapsing_header::CollapsingState, text::LayoutJob,
};
use serde_json::Value;

use super::{
    theme::{self, AMBER, BORDER, CARD, CODE_BG, CYAN, FAINT, GREEN, RED, SOFT, TEXT},
    widgets,
};
use crate::{
    app::{Approval, ApprovalKind},
    domain::{
        chat::{Message, ToolCall},
        tool::ToolKind,
    },
    tools::compress::clip,
};

pub fn tool_card(ui: &mut Ui, call: &ToolCall, result: Option<&Message>) {
    let icon = ToolKind::parse(&call.name).map_or("🔧", ToolKind::icon);
    let failed = result.is_some_and(|r| r.content.starts_with("ERROR"));
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, if failed { RED.gamma_multiply(0.4) } else { BORDER }))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let id = ui.make_persistent_id(("tool", &call.id));
            CollapsingState::load_with_default_open(ui.ctx(), id, false)
                .show_header(ui, |ui| {
                    match result {
                        None => {
                            ui.add(Spinner::new().size(12.0).color(CYAN));
                        }
                        Some(_) => widgets::dot(ui, !failed),
                    }
                    ui.label(RichText::new(format!("{icon} {}", call.name)).monospace().color(TEXT));
                    ui.label(RichText::new(summary(&call.arguments)).size(12.0).color(FAINT));
                })
                .body(|ui| {
                    ScrollArea::vertical().id_salt(("out", &call.id)).max_height(320.0).show(ui, |ui| match result {
                        None => code_block(ui, RichText::new("en cours…").monospace().color(SOFT)),
                        Some(m) => {
                            if let Some(diff) = &m.diff {
                                diff_view(ui, diff);
                            }
                            code_block(ui, RichText::new(clip(&m.content, 8000)).monospace().size(12.0).color(SOFT));
                        }
                    });
                });
        });
    ui.add_space(4.0);
}

/// Returns the user's decision once a button is clicked.
pub fn approval(ui: &mut Ui, a: &Approval) -> Option<bool> {
    let mut decision = None;
    widgets::card(ui, AMBER.gamma_multiply(0.7), |ui| {
        ui.set_width(ui.available_width());
        let title = match a.kind {
            ApprovalKind::Command => "⚠  Autorisation requise : commande",
            ApprovalKind::Edit => "✏  Valider la modification",
        };
        ui.label(RichText::new(title).color(AMBER).font(theme::bold(14.0)));
        ui.label(RichText::new(&a.reason).size(12.5).color(SOFT));
        ScrollArea::vertical().id_salt(("approval", &a.subject)).max_height(360.0).show(ui, |ui| match a.kind {
            ApprovalKind::Command => code_block(ui, RichText::new(&a.subject).monospace().color(TEXT)),
            ApprovalKind::Edit => diff_view(ui, &a.subject),
        });
        ui.horizontal(|ui| {
            if widgets::primary(ui, "✔  Autoriser", 120.0).clicked() {
                decision = Some(true);
            }
            if widgets::ghost(ui, "✖  Refuser").clicked() {
                decision = Some(false);
            }
        });
    });
    ui.add_space(6.0);
    decision
}

pub fn code_block(ui: &mut Ui, text: RichText) {
    Frame::new().fill(CODE_BG).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(Label::new(text).selectable(true));
    });
}

/// Unified diff with added/removed lines tinted.
pub fn diff_view(ui: &mut Ui, diff: &str) {
    let mut job = LayoutJob::default();
    for line in diff.lines() {
        let (color, background) = match line.as_bytes().first() {
            Some(b'+') if !line.starts_with("+++") => (GREEN, GREEN.gamma_multiply(0.10)),
            Some(b'-') if !line.starts_with("---") => (RED, RED.gamma_multiply(0.10)),
            Some(b'@') => (CYAN, Color32::TRANSPARENT),
            _ => (SOFT, Color32::TRANSPARENT),
        };
        let format = TextFormat { font_id: FontId::monospace(12.0), color, background, ..TextFormat::default() };
        job.append(line, 0.0, format.clone());
        job.append("\n", 0.0, format);
    }
    Frame::new().fill(CODE_BG).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(Label::new(job).selectable(true));
    });
    ui.add_space(4.0);
}

/// One-line digest of the tool arguments.
fn summary(arguments: &str) -> String {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(arguments) else { return clip(arguments, 80) };
    let parts: Vec<String> = map
        .values()
        .map(|v| match v {
            Value::String(s) => s.lines().next().unwrap_or_default().to_owned(),
            other => other.to_string(),
        })
        .collect();
    clip(&parts.join("  "), 90)
}
