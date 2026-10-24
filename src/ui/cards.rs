//! Tool cards, approval and error cards, code and diff blocks.

use eframe::egui::{
    Color32, CornerRadius, FontId, Frame, Label, Margin, Rect, RichText, ScrollArea, Sense, Shape, Stroke, StrokeKind,
    TextFormat, Ui, collapsing_header::CollapsingState, epaint::RectShape, pos2, text::LayoutJob, vec2,
};
use serde_json::Value;

use super::{
    fx,
    icons::{self, Icon},
    theme::{self, ACCENT_HI, AMBER, CODE_BG, CYAN, EDGE, FAINT, GREEN, MUTED, RED, SOFT, TEXT, TILE, TILE_HOVER},
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

const RADIUS: f32 = 12.0;

fn tool_icon(kind: Option<ToolKind>) -> Icon {
    match kind {
        Some(ToolKind::ListDir) => Icon::Folder,
        Some(ToolKind::ReadFile | ToolKind::Outline | ToolKind::FindSymbol) => Icon::File,
        Some(ToolKind::Grep | ToolKind::DocsSearch) => Icon::Search,
        Some(ToolKind::BlastRadius) => Icon::Graph,
        Some(ToolKind::EditFile | ToolKind::ReplaceSymbol | ToolKind::WriteFile) => Icon::Pencil,
        Some(ToolKind::Shell) => Icon::Terminal,
        Some(ToolKind::SecurityScan) => Icon::Shield,
        Some(ToolKind::WebSearch | ToolKind::WebFetch) => Icon::Globe,
        Some(ToolKind::AuditKit) => Icon::Clipboard,
        Some(ToolKind::Computer) => Icon::Eye,
        Some(ToolKind::CliAnything | ToolKind::Kits) | None => Icon::Cube,
    }
}

/// One tool call of the message at `owner`: collapsible header, output body. Returns the y of its timeline dot.
pub fn tool_card(ui: &mut Ui, owner: usize, call: &ToolCall, result: Option<&Message>, t: f64) -> f32 {
    let failed = result.is_some_and(|r| r.content.starts_with("ERROR"));
    let running = result.is_none();
    // Ids repeat across steps with some providers: the owning message keeps widget ids unique.
    let id = ui.make_persistent_id(("tool", owner, &call.id));
    let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let bg = ui.painter().add(Shape::Noop);
    let (head, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    if resp.clicked() {
        state.toggle(ui);
    }
    let open = state.openness(ui.ctx());
    let k = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    let fg = fx::mix(SOFT, TEXT, k);
    icons::chevron(p, head.left_center() + vec2(15.0, 0.0), 13.0, open, fx::mix(FAINT, TEXT, k));
    let status = head.left_center() + vec2(34.0, 0.0);
    match result {
        None => fx::spinner(p, status, 5.5, 1.8, t),
        Some(_) => widgets::status(p, status, if failed { RED } else { GREEN }, t),
    }
    icons::at(
        p,
        head.left_center() + vec2(54.0, 0.0),
        15.0,
        tool_icon(ToolKind::parse(&call.name)),
        fx::mix(MUTED, ACCENT_HI, k),
    );
    let name = widgets::line(ui, &call.name, FontId::monospace(13.0), TEXT, 220.0);
    let x = head.left() + 70.0;
    let name_w = name.size().x;
    ui.painter().galley(pos2(x, head.center().y - name.size().y / 2.0), name, TEXT);
    let summary_x = x + name_w + 12.0;
    let summary = widgets::line(ui, summary(&call.arguments), FontId::proportional(12.0), FAINT, head.right() - summary_x - 14.0);
    ui.painter().galley(pos2(summary_x, head.center().y - summary.size().y / 2.0), summary, fg.gamma_multiply(0.7));
    let body = state.show_body_unindented(ui, |ui| {
        Frame::new().inner_margin(Margin { left: 12, right: 12, top: 0, bottom: 12 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ScrollArea::vertical().id_salt(("out", owner, &call.id)).max_height(320.0).show(ui, |ui| match result {
                None => code_block(ui, RichText::new("en cours…").monospace().color(SOFT)),
                Some(m) => {
                    if let Some(diff) = &m.diff {
                        diff_view(ui, diff);
                    }
                    if m.cleared {
                        ui.label(RichText::new("Effacé du contexte du modèle (budget dépassé) · conservé ici").size(11.5).color(FAINT));
                    }
                    code_block(ui, RichText::new(clip(&m.content, 8000)).monospace().size(12.0).color(SOFT));
                }
            });
        })
    });
    let bottom = body.map_or(head.bottom(), |b| b.response.rect.bottom().max(head.bottom()));
    let card = Rect::from_min_max(head.min, pos2(head.right(), bottom));
    let accent = if failed {
        RED.gamma_multiply(0.5)
    } else if running {
        AMBER.gamma_multiply(0.45 + 0.35 * fx::pulse(t, 1.6))
    } else {
        fx::mix(EDGE, theme::EDGE_HI, k)
    };
    let mut shapes = Vec::new();
    if running {
        shapes.push(fx::halo(card, RADIUS, AMBER.gamma_multiply(0.10 + 0.08 * fx::pulse(t, 1.6)), 22));
    }
    shapes.push(RectShape::new(card, RADIUS, fx::mix(TILE, TILE_HOVER, k), Stroke::new(1.0, accent), StrokeKind::Inside).into());
    shapes.push(fx::edge_light(card, RADIUS, 26.0, 0.0));
    ui.painter().set(bg, Shape::Vec(shapes));
    ui.add_space(2.0);
    head.center().y
}

/// Returns the user's decision once a button is clicked.
pub fn approval(ui: &mut Ui, a: &Approval) -> Option<bool> {
    let mut decision = None;
    let t = fx::now(ui.ctx());
    ui.add_space(4.0);
    let bg = ui.painter().add(Shape::Noop);
    let frame = Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let (icon, title) = match a.kind {
            ApprovalKind::Command => (Icon::Alert, "Autorisation requise : commande"),
            ApprovalKind::Edit => (Icon::Pencil, "Valider la modification"),
        };
        ui.horizontal(|ui| {
            let (badge, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
            let p = ui.painter();
            fx::gradient_rect(p, badge, 10.0, AMBER.gamma_multiply(0.30), AMBER.gamma_multiply(0.10));
            icons::paint(p, badge.shrink(9.0), icon, AMBER);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new(title).color(AMBER).font(theme::bold(14.5)));
                ui.label(RichText::new(&a.reason).size(12.5).color(SOFT));
            });
        });
        ui.add_space(8.0);
        ScrollArea::vertical().id_salt(("approval", &a.subject)).max_height(360.0).show(ui, |ui| match a.kind {
            ApprovalKind::Command => code_block(ui, RichText::new(&a.subject).monospace().color(TEXT)),
            ApprovalKind::Edit => diff_view(ui, &a.subject),
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if widgets::primary_icon(ui, Some(Icon::Check), "Autoriser", 136.0).clicked() {
                decision = Some(true);
            }
            ui.add_space(4.0);
            if widgets::pill(ui, Some(Icon::Close), "Refuser", MUTED, false).clicked() {
                decision = Some(false);
            }
        });
    });
    let rect = frame.response.rect;
    let pulse = fx::pulse(t, 2.0);
    let p = ui.painter();
    let shapes = vec![
        fx::halo(rect, 16.0, AMBER.gamma_multiply(0.10 + 0.08 * pulse), 28),
        fx::gradient_fill(p, rect, 16.0, AMBER.gamma_multiply(0.09), TILE),
        RectShape::stroke(rect, 16.0, Stroke::new(1.0, AMBER.gamma_multiply(0.45 + 0.3 * pulse)), StrokeKind::Inside).into(),
    ];
    p.set(bg, Shape::Vec(shapes));
    ui.add_space(8.0);
    decision
}

/// Turn failure, tinted red.
pub fn error(ui: &mut Ui, message: &str) {
    let bg = ui.painter().add(Shape::Noop);
    let frame = Frame::new().inner_margin(Margin::same(14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            icons::paint(ui.painter(), rect, Icon::Alert, RED);
            ui.add(Label::new(RichText::new(message).size(13.0).color(Color32::from_rgb(0xFC, 0xA5, 0xA5))).wrap());
        });
    });
    let rect = frame.response.rect;
    let p = ui.painter();
    let shapes = vec![
        fx::gradient_fill(p, rect, 14.0, RED.gamma_multiply(0.14), RED.gamma_multiply(0.04)),
        RectShape::stroke(rect, 14.0, Stroke::new(1.0, RED.gamma_multiply(0.45)), StrokeKind::Inside).into(),
    ];
    p.set(bg, Shape::Vec(shapes));
}

pub fn code_block(ui: &mut Ui, text: RichText) {
    Frame::new()
        .fill(CODE_BG)
        .stroke(Stroke::new(1.0, EDGE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(Label::new(text).selectable(true));
        });
}

/// Unified diff with added/removed lines tinted.
pub fn diff_view(ui: &mut Ui, diff: &str) {
    let mut job = LayoutJob::default();
    for line in diff.lines() {
        let (color, background) = match line.as_bytes().first() {
            Some(b'+') if !line.starts_with("+++") => (GREEN, GREEN.gamma_multiply(0.12)),
            Some(b'-') if !line.starts_with("---") => (RED, RED.gamma_multiply(0.12)),
            Some(b'@') => (CYAN, Color32::TRANSPARENT),
            _ => (SOFT, Color32::TRANSPARENT),
        };
        let format = TextFormat { font_id: FontId::monospace(12.0), color, background, ..TextFormat::default() };
        job.append(line, 0.0, format.clone());
        job.append("\n", 0.0, format);
    }
    Frame::new()
        .fill(CODE_BG)
        .stroke(Stroke::new(1.0, EDGE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
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
