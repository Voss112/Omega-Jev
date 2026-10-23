//! Conversation view: bubbles, markdown, reasoning, tool cards, approvals, live stream, message actions.

use std::collections::HashMap;

use eframe::egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Label, Layout, Margin, Rect, RichText, ScrollArea, Sense, Spinner,
    Stroke, TextWrapMode, Ui, collapsing_header::CollapsingState, vec2,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use uuid::Uuid;

use super::{
    App, Live, cards,
    theme::{self, ACCENT, AMBER, BORDER, BUBBLE, CARD, CYAN, GREEN, HOVER, MUTED, RAISED, RED, TEXT},
    widgets,
};
use crate::domain::{
    chat::{Message, Role},
    route::{Route, Source},
};

/// Per-message actions, applied after rendering (the view only borrows state).
enum Action {
    Edit(usize),
    Undo(usize),
    Regenerate,
}

pub fn show(app: &mut App, ui: &mut Ui) {
    top_bar(app, ui);
    let Some(id) = app.current.filter(|id| app.conv(*id).is_some_and(|c| !c.messages.is_empty())) else {
        return empty(app, ui);
    };
    let mut action = None;
    ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| column(ui, |ui| action = body(app, id, ui)));
    match action {
        Some(Action::Edit(i)) => app.start_edit(id, i),
        Some(Action::Undo(i)) => app.undo_from(id, i),
        Some(Action::Regenerate) => app.regenerate(id),
        None => {}
    }
}

/// Centered reading column.
pub fn column(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let width = ui.available_width().min(theme::COLUMN);
    let pad = ((ui.available_width() - width) / 2.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.add_space(pad);
        ui.vertical(|ui| {
            ui.set_width(width);
            // Children of horizontal layouts default to no-wrap: force wrapping so long text never widens the column.
            ui.style_mut().wrap_mode = Some(TextWrapMode::Wrap);
            ui.add_space(16.0);
            add(ui);
            ui.add_space(20.0);
        });
    });
}

fn top_bar(app: &mut App, ui: &mut Ui) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.set_height(32.0);
        let title = app.current_conv().map_or("Nouvelle discussion", |c| c.title.as_str()).to_owned();
        ui.label(RichText::new(title).font(theme::bold(16.0)).color(TEXT));
        if let Some(conv) = app.current_conv() {
            let u = conv.usage;
            let text = format!("{} tokens · cache {:.0} %", widgets::human(u.prompt + u.completion), u.cache_ratio() * 100.0);
            widgets::chip(ui, text, MUTED);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let icon = if app.show_panel { "⏵ Outils" } else { "⏴ Outils" };
            if widgets::ghost(ui, icon).on_hover_text("Afficher/masquer le panneau des outils").clicked() {
                app.show_panel = !app.show_panel;
            }
            if let Some(id) = app.current.filter(|id| app.conv(*id).is_some_and(|c| !c.messages.is_empty()))
                && widgets::ghost(ui, "⇩ Exporter").on_hover_text("Exporter la discussion en Markdown").clicked()
            {
                app.export(id);
            }
            if let Some(n) = app.notice().map(str::to_owned) {
                widgets::chip(ui, n, AMBER);
            }
            if app.env.jev.is_none()
                && widgets::chip(ui, "⚠ Jev inactif : ajoutez la clé", RED)
                    .interact(Sense::click())
                    .on_hover_text("Sans clé Jev, le routage se fait par heuristique locale. Cliquer pour ouvrir les paramètres.")
                    .clicked()
            {
                app.settings_draft = Some(app.settings.clone());
            }
        });
    });
    let rect = ui.max_rect();
    ui.painter().hline(rect.x_range(), ui.cursor().top() + 6.0, Stroke::new(1.0, BORDER));
    ui.add_space(8.0);
}

fn body(app: &mut App, id: Uuid, ui: &mut Ui) -> Option<Action> {
    let App { convs, live, md, approvals, errors, heights, .. } = app;
    let conv = convs.iter().find(|c| c.id == id)?;
    let busy = live.contains_key(&id);
    heights.retain(|(c, _), _| *c == id);
    let results: HashMap<&str, &Message> = conv.messages.iter().filter_map(|m| Some((m.tool_call_id.as_deref()?, m))).collect();
    let last_answer = conv.messages.iter().rposition(|m| m.role == Role::Assistant);
    let width = ui.available_width();
    let mut action = None;
    let mut prev = Role::Tool;
    for (i, m) in conv.messages.iter().enumerate() {
        let first = prev == Role::User;
        prev = m.role;
        // Off-screen messages with a known height are skipped: no markdown layout, no painting.
        if let Some(&(w, h, at)) = heights.get(&(id, i))
            && at == m.turn_key()
            && (w - width).abs() < 1.0
            && !ui.is_rect_visible(Rect::from_min_size(ui.cursor().min, vec2(width, h)))
        {
            ui.add_space(h);
            continue;
        }
        let top = ui.cursor().top();
        let clicked = match m.role {
            Role::User => user(ui, i, m, busy),
            Role::Assistant => assistant(ui, md, i, m, &results, first, !busy && last_answer == Some(i)),
            Role::Tool => None,
        };
        action = action.or(clicked);
        heights.insert((id, i), (width, ui.cursor().top() - top, m.turn_key()));
    }
    if let Some(l) = live.get(&id) {
        streaming(ui, md, l, prev == Role::User);
    }
    let decision =
        approvals.iter().enumerate().filter(|(_, a)| a.conv == id).find_map(|(i, a)| Some((i, cards::approval(ui, a)?)));
    if let Some((i, ok)) = decision {
        let _ = approvals.remove(i).reply.send(ok);
    }
    if let Some(e) = errors.get(&id) {
        widgets::card(ui, RED.gamma_multiply(0.6), |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("⚠  {e}")).color(RED));
        });
    }
    action
}

fn user(ui: &mut Ui, i: usize, m: &Message, busy: bool) -> Option<Action> {
    const PAD: f32 = 28.0;
    ui.add_space(8.0);
    let text = RichText::new(&m.content).size(14.5).color(TEXT);
    let wrap = ui.available_width() * 0.8 - PAD;
    let width = ui.painter().layout(m.content.clone(), FontId::proportional(14.5), TEXT, wrap).size().x.ceil() + PAD;
    ui.horizontal_top(|ui| {
        ui.add_space((ui.available_width() - width).max(0.0));
        Frame::new()
            .fill(BUBBLE)
            .corner_radius(CornerRadius { nw: 16, ne: 4, sw: 16, se: 16 })
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_max_width(width - PAD);
                ui.add(Label::new(text).wrap().selectable(true));
            });
    });
    ui.add_space(4.0);
    // Fixed-height right-aligned row (an unbounded right-to-left layout would stretch chips vertically).
    let action = ui
        .allocate_ui_with_layout(vec2(ui.available_width(), 22.0), Layout::right_to_left(Align::Center), |ui| {
            if let Some(r) = &m.route {
                route_chips(ui, r);
            }
            if busy {
                return None;
            }
            let mut action = None;
            if m.checkpoint
                && widgets::ghost(ui, "↶")
                    .on_hover_text(
                        "Restaurer les fichiers modifiés par ce tour et les suivants (pas les effets des commandes shell)",
                    )
                    .clicked()
            {
                action = Some(Action::Undo(i));
            }
            if widgets::ghost(ui, "✏").on_hover_text("Modifier ce message et relancer à partir d'ici").clicked() {
                action = Some(Action::Edit(i));
            }
            action
        })
        .inner;
    ui.add_space(10.0);
    action
}

fn route_chips(ui: &mut Ui, r: &Route) {
    widgets::chip(ui, r.skill.label(), ACCENT);
    if let Some(p) = &r.playbook {
        widgets::chip(ui, format!("📘 {p}"), CYAN);
    }
    if r.needs_web() {
        widgets::chip(ui, "🌐 web", CYAN);
    }
    if r.needs_apps() {
        widgets::chip(ui, "🧩 apps", GREEN);
    }
    if r.risky() {
        widgets::chip(ui, "🛡 sécurité", RED);
    }
    let (source, color) = match r.source {
        Source::Jev => (format!("⚡ Jev {:.0} %", r.confidence * 100.0), CYAN),
        Source::Local => ("◌ routage local".to_owned(), MUTED),
    };
    widgets::chip(ui, source, color);
}

fn avatar_row<R>(ui: &mut Ui, first: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal_top(|ui| {
        if first {
            widgets::logo(ui, 26.0);
        } else {
            ui.add_space(26.0);
        }
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
    })
    .inner
}

fn assistant(
    ui: &mut Ui,
    md: &mut CommonMarkCache,
    i: usize,
    m: &Message,
    results: &HashMap<&str, &Message>,
    first: bool,
    can_regenerate: bool,
) -> Option<Action> {
    let action = avatar_row(ui, first, |ui| {
        if !m.reasoning.is_empty() {
            let words = m.reasoning.split_whitespace().count();
            let id = ui.make_persistent_id(("reasoning", i));
            CollapsingState::load_with_default_open(ui.ctx(), id, false)
                .show_header(ui, |ui| ui.label(RichText::new(format!("💭 Raisonnement · {words} mots")).size(12.5).color(MUTED)))
                .body(|ui| ui.label(RichText::new(&m.reasoning).size(13.0).color(MUTED).italics()));
        }
        if !m.content.trim().is_empty() {
            CommonMarkViewer::new().show(ui, md, &m.content);
        }
        for call in &m.tool_calls {
            cards::tool_card(ui, call, results.get(call.id.as_str()).copied());
        }
        (can_regenerate
            && widgets::ghost(ui, "⟳ Régénérer").on_hover_text("Annule ce tour (fichiers compris) et le relance").clicked())
        .then_some(Action::Regenerate)
    });
    ui.add_space(6.0);
    action
}

fn streaming(ui: &mut Ui, md: &mut CommonMarkCache, live: &Live, first: bool) {
    avatar_row(ui, first, |ui| {
        if !live.reasoning.is_empty() {
            Frame::new().fill(RAISED).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.add(Spinner::new().size(12.0).color(ACCENT));
                    ui.label(RichText::new("Réflexion…").size(12.5).color(MUTED));
                });
                let start = live.reasoning.char_indices().rev().nth(700).map_or(0, |(i, _)| i);
                ui.label(RichText::new(&live.reasoning[start..]).size(12.5).color(MUTED).italics());
            });
        }
        if live.text.is_empty() && live.reasoning.is_empty() {
            ui.horizontal(|ui| {
                ui.add(Spinner::new().size(14.0).color(ACCENT));
                ui.label(RichText::new("Omega travaille…").color(MUTED));
            });
        } else if !live.text.is_empty() {
            CommonMarkViewer::new().show(ui, md, &live.text);
        }
    });
}

const SUGGESTIONS: [(&str, &str, &str); 4] = [
    ("🛡", "Audit de sécurité", "Audite la sécurité de mon projet et classe les failles par sévérité."),
    ("🧪", "TDD", "Écris les tests puis implémente une fonction Rust qui valide un IBAN."),
    ("🌐", "Recherche web", "Quelles sont les nouveautés de la dernière version stable de Rust ?"),
    ("🏗", "Architecture", "Propose une architecture DDD pour une API de réservation en Rust."),
];

fn empty(app: &mut App, ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.16).max(24.0));
        widgets::logo(ui, 68.0);
        ui.add_space(16.0);
        ui.label(RichText::new("Que construisons-nous aujourd'hui ?").font(theme::bold(28.0)).color(TEXT));
        ui.add_space(4.0);
        let tagline = format!("Jev décide · {} exécute · tout reste local et anonyme", app.settings.engine.label());
        ui.label(RichText::new(tagline).color(MUTED));
        ui.add_space(28.0);
    });
    let card_w = ((ui.available_width().min(theme::COLUMN) - 12.0) / 2.0).min(330.0);
    let pad = ((ui.available_width() - card_w * 2.0 - 12.0) / 2.0).max(0.0);
    for pair in SUGGESTIONS.chunks(2) {
        ui.horizontal(|ui| {
            ui.add_space(pad);
            for (icon, title, prompt) in pair {
                if suggestion(ui, card_w, icon, title, prompt) {
                    (*prompt).clone_into(&mut app.draft);
                    app.focus_composer = true;
                }
                ui.add_space(4.0);
            }
        });
        ui.add_space(8.0);
    }
}

fn suggestion(ui: &mut Ui, width: f32, icon: &str, title: &str, prompt: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 78.0), Sense::click());
    let hovered = resp.hovered();
    let p = ui.painter();
    p.rect(
        rect,
        CornerRadius::same(14),
        if hovered { HOVER } else { CARD },
        Stroke::new(1.0, if hovered { ACCENT.gamma_multiply(0.6) } else { BORDER }),
        eframe::egui::StrokeKind::Inside,
    );
    p.text(rect.left_top() + vec2(14.0, 14.0), Align2::LEFT_TOP, format!("{icon}  {title}"), theme::bold(14.0), TEXT);
    let galley = p.layout(prompt.to_owned(), FontId::proportional(12.5), MUTED, width - 28.0);
    p.galley(rect.left_top() + vec2(14.0, 38.0), galley, Color32::WHITE);
    resp.clicked()
}
