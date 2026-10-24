//! Conversation view: bubbles, markdown, step timeline, tool cards, approvals, live stream, message actions.

use std::collections::HashMap;

use eframe::egui::{
    Align, Color32, CornerRadius, FontId, Frame, Id, Label, Layout, Margin, Pos2, Rect, RichText, ScrollArea, Sense, Shape,
    Stroke, TextWrapMode, Ui, UiBuilder, collapsing_header::CollapsingState, emath::easing, pos2, vec2,
};
use egui_commonmark::CommonMarkCache;
use uuid::Uuid;

use super::{
    App, Live,
    i18n::{english, tr}, attach, cards, files, fx,
    icons::{self, Icon},
    markdown,
    theme::{self, ACCENT, ACCENT_HI, CYAN, EDGE, EDGE_HI, FAINT, GREEN, INDIGO, MUTED, PINK, RED, SOFT, TEXT, TILE, TILE_HOVER},
    widgets,
};
use crate::domain::{
    chat::{Message, Role},
    engine::Engine,
    models,
    route::{Route, Source},
};

/// Width of the avatar / timeline gutter left of assistant messages.
const GUTTER: f32 = 40.0;
/// Vertical gap the timeline bridges between two steps.
const BRIDGE: f32 = 16.0;

/// Per-message actions, applied after rendering (the view only borrows state).
enum Action {
    Edit(usize),
    Undo(usize),
    Regenerate,
}

/// Where an assistant message sits in its turn, for the step timeline.
#[derive(Clone, Copy)]
struct Step {
    /// First assistant message after the user prompt: carries the avatar.
    first: bool,
    /// Index within the turn, drives the violet → cyan colour.
    index: usize,
    /// Another step follows (tool results feed a next message).
    more: bool,
}

impl Step {
    fn color(self) -> Color32 {
        fx::mix(ACCENT_HI, CYAN, self.index as f32 / 6.0)
    }
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
    ui.add_space(4.0);
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
    let mut right = ui.new_child(UiBuilder::new().max_rect(bar).layout(Layout::right_to_left(Align::Center)));
    right.spacing_mut().item_spacing.x = 4.0;
    if widgets::pill(&mut right, Some(Icon::PanelRight), tr("Outils", "Tools"), MUTED, app.show_panel)
        .on_hover_text(tr("Afficher/masquer le panneau des outils", "Show/hide the tools panel"))
        .clicked()
    {
        app.show_panel = !app.show_panel;
    }
    let pending = app.jobs.iter().filter(|j| j.pending() && !j.paused).count();
    let tasks = if pending > 0 { format!("{} ({pending})", tr("Tâches", "Tasks")) } else { tr("Tâches", "Tasks").to_owned() };
    if widgets::pill(&mut right, Some(Icon::Clock), &tasks, MUTED, app.show_tasks)
        .on_hover_text(tr("Toutes les tâches programmées (/tasks)", "All scheduled tasks (/tasks)"))
        .clicked()
    {
        app.show_tasks = !app.show_tasks;
    }
    if widgets::pill(&mut right, Some(Icon::PanelLeft), tr("Fichiers", "Files"), MUTED, app.files.show)
        .on_hover_text(tr("Arborescence du dossier de travail", "Workspace file tree"))
        .clicked()
    {
        files::toggle(app);
    }
    if let Some(id) = app.current.filter(|id| app.conv(*id).is_some_and(|c| !c.messages.is_empty()))
        && widgets::pill(&mut right, Some(Icon::Download), tr("Exporter", "Export"), MUTED, false)
            .on_hover_text(tr("Exporter la discussion en Markdown", "Export the chat as Markdown"))
            .clicked()
    {
        app.export(id);
    }
    if app.env.jev.is_none() {
        right.add_space(6.0);
        if widgets::chip_icon(&mut right, Some(Icon::Alert), tr("Jev inactif : ajoutez la clé", "Jev inactive: add the key"), RED)
            .interact(Sense::click())
            .on_hover_text(tr(
                "Sans clé Jev, le routage se fait par heuristique locale. Cliquer pour ouvrir les paramètres.",
                "Without a Jev key, routing uses local heuristics. Click to open the settings.",
            ))
            .on_hover_cursor(eframe::egui::CursorIcon::PointingHand)
            .clicked()
        {
            app.settings_draft = Some(app.settings.clone());
        }
    }
    let used = bar.right() - right.min_rect().left();
    let left_rect = Rect::from_min_max(bar.min, pos2(bar.right() - used - 16.0, bar.bottom()));
    let mut left = ui.new_child(UiBuilder::new().max_rect(left_rect).layout(Layout::left_to_right(Align::Center)));
    left.spacing_mut().item_spacing.x = 8.0;
    let (engine, effort) = (app.settings.engine, app.settings.effort.get(app.settings.engine));
    let model = app.settings.model(engine).to_owned();
    let model_name = models::name(engine, &model).to_owned();
    let usage = app.current_conv().map(|c| {
        let u = c.usage;
        format!("{} tokens · cache {:.0} %", widgets::human(u.prompt + u.completion), u.cache_ratio() * 100.0)
    });
    let title = app.current_conv().map_or(tr("Nouvelle discussion", "New chat"), |c| c.title.as_str()).to_owned();
    // The title keeps priority: the usage chip (also in the tools panel), then the model chip, give way first.
    let natural = left.painter().layout_no_wrap(title.clone(), theme::bold(15.5), TEXT).size().x;
    let wanted = natural.min(260.0);
    let model_w = text_w(&left, &model_name) + 45.0;
    let usage_w = usage.as_deref().map_or(0.0, |u| text_w(&left, u) + 26.0);
    let avail = left_rect.width();
    let show_model = wanted + model_w <= avail;
    let usage = usage.filter(|_| show_model && wanted + model_w + usage_w <= avail);
    let title_w = avail - if show_model { model_w } else { 0.0 } - if usage.is_some() { usage_w } else { 0.0 };
    let galley = widgets::line(&left, &title, theme::bold(15.5), TEXT, title_w);
    let elided = galley.elided;
    let (rect, resp) = left.allocate_exact_size(galley.size(), Sense::hover());
    left.painter().galley(rect.min, galley, TEXT);
    if elided {
        resp.on_hover_text(&title);
    }
    left.add_space(4.0);
    if show_model {
        let tip = if english() {
            format!("{} · model {model} · effort {}\nUse the picker in the input box to change", engine.label(), effort.label())
        } else {
            format!(
                "{} · modèle {model} · effort {}\nCliquer sur le sélecteur de la zone de saisie pour changer",
                engine.label(),
                effort.label()
            )
        };
        widgets::chip_icon(&mut left, Some(Icon::Bolt), model_name, CYAN).on_hover_text(tip);
    }
    if let Some(u) = usage {
        widgets::chip(&mut left, u, MUTED);
    }
    fx::hairline(ui.painter(), bar.left(), bar.right(), bar.bottom() + 2.0, EDGE_HI);
    ui.add_space(6.0);
}

fn text_w(ui: &Ui, text: &str) -> f32 {
    ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(11.5), TEXT).size().x
}

fn body(app: &mut App, id: Uuid, ui: &mut Ui) -> Option<Action> {
    let App { convs, live, md, approvals, errors, heights, tex, .. } = app;
    let conv = convs.iter().find(|c| c.id == id)?;
    let busy = live.contains_key(&id);
    heights.retain(|(c, _), _| *c == id);
    // A result answers a call of the assistant message it follows: ids repeat across steps (older OpenAI-compatible engines).
    let mut results: HashMap<(usize, &str), &Message> = HashMap::new();
    let mut owner = 0;
    for (i, m) in conv.messages.iter().enumerate() {
        match (m.role, m.tool_call_id.as_deref()) {
            (Role::Assistant, _) => owner = i,
            (Role::Tool, Some(call)) => {
                results.insert((owner, call), m);
            }
            _ => {}
        }
    }
    let last_answer = conv.messages.iter().rposition(|m| m.role == Role::Assistant);
    let width = ui.available_width();
    let t = fx::now(ui.ctx());
    let mut action = None;
    let mut prev = Role::Tool;
    let mut index = 0;
    for (i, m) in conv.messages.iter().enumerate() {
        let first = prev == Role::User;
        prev = m.role;
        let step = Step { first, index, more: !m.tool_calls.is_empty() };
        match m.role {
            Role::User => index = 0,
            Role::Assistant => index += 1,
            Role::Tool => {}
        }
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
            Role::User => appear(ui, (id, i, m.turn_key()), t, |ui| user(ui, i, m, busy, tex)),
            Role::Assistant => appear(ui, (id, i, m.turn_key()), t, |ui| {
                assistant(ui, md, i, m, &results, step, !busy && last_answer == Some(i), t)
            }),
            Role::Tool => None,
        };
        action = action.or(clicked);
        heights.insert((id, i), (width, ui.cursor().top() - top, m.turn_key()));
    }
    if let Some(l) = live.get(&id) {
        streaming(ui, md, l, Step { first: prev == Role::User, index, more: false }, t);
    }
    let decision =
        approvals.iter().enumerate().filter(|(_, a)| a.conv == id).find_map(|(i, a)| Some((i, cards::approval(ui, a)?)));
    if let Some((i, ok)) = decision {
        let _ = approvals.remove(i).reply.send(ok);
    }
    if let Some(e) = errors.get(&id) {
        cards::error(ui, e);
    }
    action
}

/// Fades a message in the first time it is shown.
fn appear<R>(ui: &mut Ui, key: (Uuid, usize, i64), t: f64, add: impl FnOnce(&mut Ui) -> R) -> R {
    let id = Id::unique(("appear", key));
    let born = ui.data_mut(|d| *d.get_temp_mut_or_insert_with(id, || t));
    let k = easing::cubic_out((((t - born) / 0.4) as f32).clamp(0.0, 1.0));
    ui.scope(|ui| {
        if k < 1.0 {
            ui.multiply_opacity(k);
        }
        add(ui)
    })
    .inner
}

fn user(ui: &mut Ui, i: usize, m: &Message, busy: bool, tex: &mut HashMap<u64, eframe::egui::TextureHandle>) -> Option<Action> {
    const PAD: f32 = 32.0;
    ui.add_space(10.0);
    let column = ui.max_rect().x_range();
    let top = ui.cursor().top();
    let text = RichText::new(&m.content).size(14.5).color(TEXT);
    let wrap = ui.available_width() * 0.78 - PAD;
    let text_w = if m.content.trim().is_empty() {
        0.0
    } else {
        ui.painter().layout(m.content.clone(), FontId::proportional(14.5), TEXT, wrap).size().x.ceil()
    };
    // Thumbnails sit in one wrapped row above the text: the bubble is at least as wide as that row.
    const THUMB: f32 = 150.0;
    let images_w = if m.images.is_empty() { 0.0 } else { m.images.len() as f32 * (THUMB + 8.0) - 8.0 };
    let width = (text_w.max(images_w) + PAD).min(ui.available_width() * 0.85);
    let bubble = ui
        .horizontal_top(|ui| {
            ui.add_space((ui.available_width() - width).max(0.0));
            let bg = ui.painter().add(Shape::Noop);
            let frame = Frame::new().inner_margin(Margin::symmetric(16, 11)).show(ui, |ui| {
                // The frame inherits the row's left-to-right layout: stack its content explicitly.
                ui.vertical(|ui| {
                    ui.set_max_width(width - PAD);
                    if !m.images.is_empty() {
                        let ctx = ui.ctx().clone();
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                            for img in &m.images {
                                attach::thumb(ui, &ctx, tex, img, THUMB.min(width - PAD), false);
                            }
                        });
                        if !m.content.trim().is_empty() {
                            ui.add_space(8.0);
                        }
                    }
                    if !m.content.trim().is_empty() {
                        ui.add(Label::new(text).wrap().selectable(true));
                    }
                });
            });
            let rect = frame.response.rect;
            let radius = CornerRadius { nw: 18, ne: 6, sw: 18, se: 18 };
            let p = ui.painter();
            let shapes = vec![
                fx::halo(rect, radius, ACCENT.gamma_multiply(0.16), 26),
                fx::gradient_fill(p, rect, radius, ACCENT.gamma_multiply(0.50), INDIGO.gamma_multiply(0.34)),
                fx::edge_light(rect, radius, 64.0, 16.0),
            ];
            p.set(bg, Shape::Vec(shapes));
            rect
        })
        .inner;
    ui.add_space(4.0);
    let zone = Rect::from_x_y_ranges(column, top..=bubble.bottom() + 28.0);
    let reveal = ui.ctx().animate_bool_with_time(Id::unique(("user-actions", i)), ui.rect_contains_pointer(zone), 0.15);
    // Fixed-height right-aligned row (an unbounded right-to-left layout would stretch chips vertically).
    let action = ui
        .allocate_ui_with_layout(vec2(ui.available_width(), 24.0), Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if let Some(r) = &m.route {
                route_chips(ui, r);
            }
            if busy {
                return None;
            }
            ui.add_space(4.0);
            ui.scope(|ui| {
                ui.multiply_opacity(reveal);
                let mut action = None;
                if m.checkpoint
                    && widgets::icon_button_sized(ui, Icon::Undo, 26.0, MUTED, false)
                        .on_hover_text(
                            tr(
                                "Restaurer les fichiers modifiés par ce tour et les suivants (pas les effets des commandes shell)",
                                "Restore the files changed by this turn and the next ones (not shell command effects)",
                            ),
                        )
                        .clicked()
                {
                    action = Some(Action::Undo(i));
                }
                if widgets::icon_button_sized(ui, Icon::Pencil, 26.0, MUTED, false)
                    .on_hover_text(tr("Modifier ce message et relancer à partir d'ici", "Edit this message and rerun from here"))
                    .clicked()
                {
                    action = Some(Action::Edit(i));
                }
                action
            })
            .inner
        })
        .inner;
    ui.add_space(12.0);
    action
}

fn route_chips(ui: &mut Ui, r: &Route) {
    let (source, color) = match r.source {
        Source::Jev => (format!("Jev {:.0} %", r.confidence * 100.0), CYAN),
        Source::Local => (tr("routage local", "local routing").to_owned(), MUTED),
    };
    widgets::chip_icon(ui, Some(if r.source == Source::Jev { Icon::Bolt } else { Icon::Gear }), source, color);
    if r.risky() {
        widgets::chip_icon(ui, Some(Icon::Shield), tr("sécurité", "security"), RED);
    }
    if r.needs_apps() {
        widgets::chip_icon(ui, Some(Icon::Cube), "apps", GREEN);
    }
    if r.needs_web() {
        widgets::chip_icon(ui, Some(Icon::Globe), "web", CYAN);
    }
    if let Some(p) = &r.playbook {
        widgets::chip_icon(ui, Some(Icon::Book), p.clone(), CYAN);
    }
    widgets::chip_icon(ui, Some(Icon::Sparkle), r.skill.label(), ACCENT_HI);
}

/// Gutter (avatar or timeline) + content column. Returns the content's result and the dot positions.
fn stepped<R>(ui: &mut Ui, step: Step, t: f64, add: impl FnOnce(&mut Ui, &mut Vec<f32>) -> R) -> R {
    let top = ui.cursor().top();
    let left = ui.cursor().left();
    let mut dots = Vec::new();
    let inner = ui
        .horizontal_top(|ui| {
            ui.add_space(GUTTER);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                add(ui, &mut dots)
            })
            .inner
        })
        .inner;
    let bottom = ui.cursor().top();
    let x = left + 14.0;
    let p = ui.painter();
    let color = step.color();
    if step.first {
        let avatar = Rect::from_min_size(pos2(left, top), vec2(28.0, 28.0));
        widgets::paint_logo(p, avatar, t);
    }
    let start = if step.first { top + 34.0 } else { top - BRIDGE };
    let end = if step.more { bottom + 2.0 } else { dots.last().copied().unwrap_or(start) };
    if end > start {
        let next = fx::mix(ACCENT_HI, CYAN, (step.index + 1) as f32 / 6.0);
        let line = Rect::from_min_max(pos2(x - 0.75, start), pos2(x + 0.75, end));
        fx::paint_with(
            p,
            eframe::egui::epaint::RectShape::filled(line, 0.0, Color32::WHITE),
            fx::linear(line, color.gamma_multiply(0.45), next.gamma_multiply(0.45), vec2(0.0, 1.0)),
        );
    }
    for y in dots {
        let c = pos2(x, y);
        p.circle_filled(c, 7.0, fx::additive(color.gamma_multiply(0.18)));
        p.circle_filled(c, 4.0, theme::BG);
        p.circle_stroke(c, 4.0, Stroke::new(1.5, color));
        p.circle_filled(c, 1.6, color);
    }
    inner
}

#[allow(clippy::too_many_arguments)]
fn assistant(
    ui: &mut Ui,
    md: &mut CommonMarkCache,
    i: usize,
    m: &Message,
    results: &HashMap<(usize, &str), &Message>,
    step: Step,
    can_regenerate: bool,
    t: f64,
) -> Option<Action> {
    let action = stepped(ui, step, t, |ui, dots| {
        if step.first {
            header(ui, (!m.model.is_empty()).then(|| format!("{} · {}", m.engine.map_or("", Engine::label), m.model)));
        }
        if !m.reasoning.is_empty() {
            dots.push(reasoning(ui, i, m));
        }
        if !m.content.trim().is_empty() {
            dots.push(ui.cursor().top() + 11.0);
            markdown::show(ui, md, &m.content);
        }
        for call in &m.tool_calls {
            dots.push(cards::tool_card(ui, i, call, results.get(&(i, call.id.as_str())).copied(), t));
        }
        if can_regenerate {
            ui.add_space(2.0);
            return widgets::pill(ui, Some(Icon::Refresh), tr("Régénérer", "Regenerate"), MUTED, false)
                .on_hover_text(tr("Annule ce tour (fichiers compris) et le relance", "Undo this turn (files included) and run it again"))
                .clicked()
                .then_some(Action::Regenerate);
        }
        None
    });
    ui.add_space(6.0);
    action
}

/// "Omega" + the exact model, beside the avatar.
fn header(ui: &mut Ui, model: Option<String>) {
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 28.0), Layout::left_to_right(Align::Center), |ui| {
        ui.label(RichText::new("Omega").font(theme::bold(13.5)).color(TEXT));
        if let Some(model) = model {
            ui.label(RichText::new(model).size(10.5).monospace().color(FAINT));
        }
    });
    ui.add_space(2.0);
}

/// Collapsible reasoning row; returns the y of its timeline dot.
fn reasoning(ui: &mut Ui, i: usize, m: &Message) -> f32 {
    let words = m.reasoning.split_whitespace().count();
    let id = ui.make_persistent_id(("reasoning", i));
    let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    if resp.clicked() {
        state.toggle(ui);
    }
    let open = state.openness(ui.ctx());
    let k = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    if k > 0.0 {
        p.rect_filled(rect, 9.0, TILE_HOVER.gamma_multiply(k));
    }
    let fg = fx::mix(MUTED, TEXT, k);
    icons::chevron(p, rect.left_center() + vec2(12.0, 0.0), 13.0, open, fg);
    icons::at(p, rect.left_center() + vec2(32.0, 0.0), 15.0, Icon::Bubble, fx::mix(MUTED, ACCENT_HI, k));
    let label = widgets::line(ui, format!("{} · {words} {}", tr("Raisonnement", "Reasoning"), tr("mots", "words")), FontId::proportional(12.5), fg, rect.width() - 60.0);
    ui.painter().galley(rect.left_center() + vec2(46.0, -label.size().y / 2.0), label, fg);
    state.show_body_unindented(ui, |ui| {
        let frame = Frame::new().inner_margin(Margin { left: 16, right: 12, top: 4, bottom: 8 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(&m.reasoning).size(13.0).color(MUTED).italics());
        });
        let r = frame.response.rect;
        ui.painter().line_segment(
            [pos2(r.left() + 4.0, r.top() + 4.0), pos2(r.left() + 4.0, r.bottom() - 6.0)],
            Stroke::new(2.0, ACCENT.gamma_multiply(0.35)),
        );
    });
    rect.center().y
}

fn streaming(ui: &mut Ui, md: &mut CommonMarkCache, live: &Live, step: Step, t: f64) {
    stepped(ui, step, t, |ui, dots| {
        if step.first {
            header(ui, None);
        }
        if !live.reasoning.is_empty() {
            let frame = Frame::new()
                .fill(TILE)
                .stroke(Stroke::new(1.0, EDGE))
                .corner_radius(CornerRadius::same(14))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
                    fx::spinner(ui.painter(), rect.left_center() + vec2(8.0, 0.0), 6.0, 2.0, t);
                    let job = fx::shimmer(tr("Réflexion…", "Thinking…"), FontId::proportional(12.5), MUTED, TEXT, t);
                    let galley = ui.painter().layout_job(job);
                    ui.painter().galley(rect.left_center() + vec2(24.0, -galley.size().y / 2.0), galley, MUTED);
                    ui.add_space(4.0);
                    let start = live.reasoning.char_indices().rev().nth(700).map_or(0, |(i, _)| i);
                    ui.label(RichText::new(&live.reasoning[start..]).size(12.5).color(MUTED).italics());
                });
            dots.push(frame.response.rect.top() + 21.0);
        }
        if live.text.is_empty() && live.reasoning.is_empty() {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
            fx::spinner(ui.painter(), rect.left_center() + vec2(10.0, 0.0), 7.0, 2.2, t);
            let job = fx::shimmer(tr("Omega travaille…", "Omega is working…"), FontId::proportional(13.5), MUTED, Color32::WHITE, t);
            let galley = ui.painter().layout_job(job);
            ui.painter().galley(rect.left_center() + vec2(28.0, -galley.size().y / 2.0), galley, MUTED);
            dots.push(rect.center().y);
        } else if !live.text.is_empty() {
            dots.push(ui.cursor().top() + 11.0);
            markdown::show(ui, md, &live.text);
        }
    });
}

fn suggestions() -> [(Icon, Color32, &'static str, &'static str); 4] {
    [
        (
            Icon::Shield,
            RED,
            tr("Audit de sécurité", "Security audit"),
            tr(
                "Audite la sécurité de mon projet et classe les failles par sévérité.",
                "Audit my project's security and rank the flaws by severity.",
            ),
        ),
        (
            Icon::Flask,
            GREEN,
            "TDD",
            tr(
                "Écris les tests puis implémente une fonction Rust qui valide un IBAN.",
                "Write the tests, then implement a Rust function that validates an IBAN.",
            ),
        ),
        (
            Icon::Globe,
            CYAN,
            tr("Recherche web", "Web search"),
            tr("Quelles sont les nouveautés de la dernière version stable de Rust ?", "What is new in the latest stable Rust release?"),
        ),
        (
            Icon::Layers,
            PINK,
            "Architecture",
            tr(
                "Propose une architecture DDD pour une API de réservation en Rust.",
                "Propose a DDD architecture for a booking API in Rust.",
            ),
        ),
    ]
}

fn empty(app: &mut App, ui: &mut Ui) {
    let t = fx::now(ui.ctx());
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.13).max(20.0));
        let (rect, _) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::hover());
        widgets::paint_logo(ui.painter(), rect.translate(vec2(0.0, -3.0 * fx::pulse(t, 5.0))), t);
        ui.add_space(22.0);
        let title = tr("Que construisons-nous aujourd'hui ?", "What are we building today?");
        let from = Color32::from_rgb(0xED, 0xE9, 0xFE);
        ui.label(fx::gradient_text(title, theme::bold(30.0), from, Color32::from_rgb(0xA5, 0xF3, 0xFC)));
        ui.add_space(6.0);
        let (engine, effort) = (app.settings.engine, app.settings.effort.get(app.settings.engine));
        let model = app.settings.model(engine).to_owned();
        let tagline = if english() {
            format!(
                "Jev decides · {} ({model}) executes (effort {}) · everything stays local and anonymous",
                models::name(engine, &model),
                effort.label()
            )
        } else {
            format!(
                "Jev décide · {} ({model}) exécute (effort {}) · tout reste local et anonyme",
                models::name(engine, &model),
                effort.label()
            )
        };
        ui.label(RichText::new(tagline).size(14.0).color(MUTED));
        ui.add_space(32.0);
    });
    let gap = 14.0;
    let card_w = ((ui.available_width().min(theme::COLUMN) - gap) / 2.0).min(340.0);
    let (grid, _) = ui.allocate_exact_size(vec2(ui.available_width(), 92.0 * 2.0 + gap), Sense::hover());
    let left = grid.center().x - card_w - gap / 2.0;
    for (i, (icon, color, title, prompt)) in suggestions().iter().enumerate() {
        let x = left + (i % 2) as f32 * (card_w + gap);
        let y = grid.top() + (i / 2) as f32 * (92.0 + gap);
        let rect = Rect::from_min_size(pos2(x, y), vec2(card_w, 92.0));
        if suggestion(ui, rect, *icon, *color, title, prompt) {
            (*prompt).clone_into(&mut app.draft);
            app.focus_composer = true;
        }
    }
}

fn suggestion(ui: &mut Ui, rect: Rect, icon: Icon, color: Color32, title: &str, prompt: &str) -> bool {
    let resp = ui.interact(rect, Id::unique(("suggestion", title)), Sense::click());
    let k = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.2);
    let rect = rect.translate(vec2(0.0, -3.0 * k));
    let p = ui.painter();
    fx::shadow(p, rect, 16.0, (60.0 + 60.0 * k) as u8);
    if k > 0.0 {
        fx::glow(p, rect, 16.0, color.gamma_multiply(0.16 * k), 30);
    }
    fx::glass(p, rect, 16.0, fx::mix(theme::GLASS, TILE_HOVER, k * 0.5));
    if k > 0.0 {
        fx::gradient_border(
            p,
            rect,
            16.0,
            1.0,
            fx::linear(rect, color.gamma_multiply(0.7 * k), Color32::TRANSPARENT, vec2(1.0, 1.0)),
        );
    }
    let badge = Rect::from_min_size(rect.left_top() + vec2(16.0, 16.0), vec2(36.0, 36.0));
    fx::gradient_rect(p, badge, 11.0, color.gamma_multiply(0.32), color.gamma_multiply(0.10));
    p.rect_stroke(badge, 11.0, Stroke::new(1.0, color.gamma_multiply(0.35)), eframe::egui::StrokeKind::Inside);
    icons::paint(p, badge.shrink(9.0), icon, color);
    let x = badge.right() + 14.0;
    let text_w = rect.right() - x - 16.0;
    let head = widgets::line(ui, title, theme::bold(14.0), TEXT, text_w);
    let p = ui.painter();
    p.galley(Pos2::new(x, rect.top() + 17.0), head, TEXT);
    let mut job =
        eframe::egui::text::LayoutJob::simple(prompt.to_owned(), FontId::proportional(12.5), fx::mix(MUTED, SOFT, k), text_w);
    job.wrap.max_rows = 2;
    let body = p.layout_job(job);
    p.galley(Pos2::new(x, rect.top() + 40.0), body, MUTED);
    icons::at(
        p,
        rect.right_top() + vec2(-20.0, 22.0),
        13.0,
        Icon::ArrowUp,
        fx::mix(FAINT, color, k).gamma_multiply(0.4 + 0.6 * k),
    );
    resp.clicked()
}
