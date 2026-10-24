//! Prompt box: Enter sends, Shift+Enter new line, engine/model/effort pickers, send/stop.

use eframe::egui::{
    Align, Align2, Color32, FontId, Frame, Id, Key, Layout, Margin, Modifiers, Rect, ScrollArea, Sense, Shape, Stroke, StrokeKind,
    TextEdit, Ui, epaint::RectShape, pos2, vec2,
};

use super::{
    App, attach,
    chat::column,
    dictation, fx,
    icons::{self, Icon},
    theme::{self, ACCENT, ACCENT_HI, AMBER, CYAN, FAINT, INDIGO, MUTED, RED, SOFT, TEXT},
    widgets,
};
use crate::domain::{
    auth::AuthMode,
    engine::{Effort, Engine},
    models,
    settings::Settings,
};

const RADIUS: f32 = 22.0;
const SEND: f32 = 38.0;

pub fn show(app: &mut App, ui: &mut Ui) {
    let id = Id::unique("composer");
    let focused = ui.memory(|m| m.has_focus(id));
    let busy = app.busy();
    let mut send = focused && !busy && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
    let mut stop = false;
    let mut cancel_edit = false;
    let mut schedule = false;
    let mut pick = false;
    let mut dictate = false;
    let ctx = ui.ctx().clone();
    let t = fx::now(&ctx);
    app.collect_images(&ctx);
    column(ui, |ui| {
        if app.editing.is_some() {
            cancel_edit = editing_banner(ui);
            ui.add_space(6.0);
        }
        let bg = ui.painter().add(Shape::Noop);
        let frame = Frame::new().inner_margin(Margin { left: 18, right: 12, top: 14, bottom: 10 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !app.shots.is_empty() {
                let (shots, tex) = (&app.shots, &mut app.tex);
                let mut remove = None;
                ui.horizontal_wrapped(|ui| {
                    let ctx = ui.ctx().clone();
                    for (i, img) in shots.iter().enumerate() {
                        if attach::thumb(ui, &ctx, tex, img, 64.0, true) {
                            remove = Some(i);
                        }
                    }
                });
                if let Some(i) = remove {
                    app.shots.remove(i);
                }
                ui.add_space(6.0);
            }
            ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                let edit = TextEdit::multiline(&mut app.draft)
                    .id(id)
                    .hint_text("Demandez n'importe quoi…")
                    .frame(Frame::NONE)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .font(FontId::proportional(15.0))
                    .text_color(TEXT);
                let resp = ui.add(edit);
                if app.focus_composer {
                    resp.request_focus();
                    app.focus_composer = false;
                }
            });
            ui.add_space(6.0);
            let toolbar = ui
                .allocate_ui_with_layout(
                    vec2(ui.available_width() - SEND - 10.0, 30.0),
                    Layout::left_to_right(Align::Center).with_main_wrap(true),
                    |ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        let compact = ui.available_width() < 760.0;
                        if widgets::icon_button(ui, Icon::Image, "Joindre une image — glisser-déposer et Ctrl+V marchent aussi")
                            .clicked()
                        {
                            pick = true;
                        }
                        dictate = dictation::button(app, ui, t);
                        engine_picker(app, ui);
                        bypass_toggle(app, ui, t);
                        if widgets::pill(ui, Some(Icon::Clock), if compact { "" } else { "Programmer" }, MUTED, false)
                            .on_hover_text("Envoyer ce prompt plus tard, avec le moteur et l'effort choisis (toujours en bypass)")
                            .clicked()
                        {
                            schedule = true;
                        }
                    },
                )
                .response
                .rect;
            let right = ui.max_rect().right();
            let button = Rect::from_center_size(pos2(right - SEND / 2.0, toolbar.bottom() - 15.0), vec2(SEND, SEND));
            let clicked = action_button(ui, button, busy, !app.draft.trim().is_empty() || !app.shots.is_empty(), t);
            send |= clicked && !busy;
            stop |= clicked && busy;
            if app.model_input.is_some() {
                ui.add_space(6.0);
                custom_model(app, ui);
            }
        });
        let rect = frame.response.rect;
        let k = ui.ctx().animate_bool_with_time(id.with("focus"), focused, 0.25);
        let p = ui.painter();
        let mut shapes = vec![
            Shape::Rect(
                eframe::egui::Shadow { offset: [0, 12], blur: 36, spread: 0, color: Color32::from_black_alpha(130) }
                    .as_shape(rect, RADIUS),
            ),
            fx::halo(rect, RADIUS, fx::mix(ACCENT, CYAN, 0.3).gamma_multiply(0.06 + 0.16 * k), (20.0 + 18.0 * k) as u8),
            RectShape::filled(rect, RADIUS, theme::rgba(14, 16, 28, 214)).into(),
            fx::edge_light(rect, RADIUS, 44.0, 16.0),
        ];
        if k > 0.0 {
            shapes.push(fx::border(rect, RADIUS, 1.5, fx::conic(rect, t, 0.95 * k)));
        }
        if ui.ctx().input(|i| !i.raw.hovered_files.is_empty()) {
            shapes.push(Shape::rect_stroke(rect, RADIUS, Stroke::new(2.0, CYAN), StrokeKind::Inside));
            p.text(rect.center(), Align2::CENTER_CENTER, "Déposez l'image ici", FontId::proportional(14.0), CYAN);
        }
        p.set(bg, Shape::Vec(shapes));
        ui.add_space(8.0);
        hints(ui, app.settings.style.label());
    });
    if cancel_edit {
        app.cancel_edit();
    }
    if pick {
        let picked = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "bmp"]).pick_files();
        for path in picked.unwrap_or_default() {
            match attach::from_path(&path) {
                Ok(img) => app.attach(img),
                Err(e) => app.notify(e),
            }
        }
    }
    if schedule {
        app.open_schedule();
    }
    if dictate {
        app.toggle_dictation(&ctx);
    }
    if send && app.dictating() {
        app.send_after_dictation();
    } else if send {
        app.send();
    }
    if stop && let Some(id) = app.current {
        app.stop(id);
    }
}

/// Amber strip shown while a sent message is being edited; returns `true` on cancel.
fn editing_banner(ui: &mut Ui) -> bool {
    let mut cancel = false;
    let bg = ui.painter().add(Shape::Noop);
    let frame = Frame::new().inner_margin(Margin { left: 12, right: 6, top: 4, bottom: 4 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), rect, Icon::Pencil, AMBER);
            let width = (ui.available_width() - 150.0).max(60.0);
            ui.allocate_ui(vec2(width, 30.0), |ui| {
                widgets::truncated(
                    ui,
                    "Modification : les messages suivants et leurs changements de fichiers seront annulés à l'envoi.",
                    FontId::proportional(12.5),
                    AMBER,
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                cancel = widgets::pill(ui, Some(Icon::Close), "Annuler (Échap)", SOFT, false).clicked();
            });
        });
    });
    let rect = frame.response.rect;
    let p = ui.painter();
    p.set(
        bg,
        Shape::Vec(vec![
            fx::gradient_fill(p, rect, 12.0, AMBER.gamma_multiply(0.13), AMBER.gamma_multiply(0.04)),
            RectShape::stroke(rect, 12.0, Stroke::new(1.0, AMBER.gamma_multiply(0.35)), StrokeKind::Inside).into(),
        ]),
    );
    cancel
}

/// Keyboard hints under the box; trailing items drop out when the column is narrow.
fn hints(ui: &mut Ui, style: &str) {
    let items: [(&str, &str); 6] = [
        ("Entrée", "envoyer"),
        ("Maj+Entrée", "nouvelle ligne"),
        ("Ctrl+M", "dicter"),
        ("Ctrl+N", "nouvelle discussion"),
        ("Échap", "stop"),
        ("/clear", "vider le contexte"),
    ];
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    let font = FontId::proportional(11.0);
    let key_font = FontId::monospace(10.0);
    let p = ui.painter();
    let mut pieces = Vec::new();
    let mut total = 0.0;
    let tail = p.layout_no_wrap(format!("Style {style}  ·  Omega peut se tromper"), font.clone(), FAINT);
    for (key, what) in items {
        let k = p.layout_no_wrap(key.to_owned(), key_font.clone(), MUTED);
        let w = p.layout_no_wrap(what.to_owned(), font.clone(), FAINT);
        let width = k.size().x + 10.0 + 5.0 + w.size().x + 16.0;
        if total + width > row.width() {
            break;
        }
        total += width;
        pieces.push((k, w));
    }
    let show_tail = total + tail.size().x <= row.width();
    if show_tail {
        total += tail.size().x;
    }
    let mut x = row.center().x - total / 2.0;
    let cy = row.center().y;
    for (k, w) in pieces {
        let cap = Rect::from_min_size(pos2(x, cy - 8.0), vec2(k.size().x + 10.0, 16.0));
        p.rect(cap, 4.0, theme::TILE, Stroke::new(1.0, theme::EDGE_HI), StrokeKind::Inside);
        let kw = k.size();
        p.galley(cap.center() - kw / 2.0, k, MUTED);
        x = cap.right() + 5.0;
        let ww = w.size();
        p.galley(pos2(x, cy - ww.y / 2.0), w, FAINT);
        x += ww.x + 16.0;
    }
    if show_tail {
        let size = tail.size();
        p.galley(pos2(x, cy - size.y / 2.0), tail, FAINT);
    }
}

/// Bypass permissions: one click, clearly red while active.
fn bypass_toggle(app: &mut App, ui: &mut Ui, t: f64) {
    let on = app.settings.bypass;
    let tip = if on {
        "Bypass actif : commandes, installations et modifications sont acceptées sans demander (les commandes destructrices restent bloquées). Cliquer pour revenir aux demandes."
    } else {
        "Cliquer pour accepter automatiquement toutes les autorisations (mode Bypass)."
    };
    let resp = if on {
        let galley = widgets::line(ui, "Bypass", FontId::proportional(13.0), RED, 200.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 44.0, 30.0), Sense::click());
        let k = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.14);
        let p = ui.painter();
        fx::glow(p, rect, 10.0, RED.gamma_multiply(0.10 + 0.08 * fx::pulse(t, 2.4) + 0.1 * k), 16);
        p.rect(rect, 10.0, RED.gamma_multiply(0.13 + 0.06 * k), Stroke::new(1.0, RED.gamma_multiply(0.45)), StrokeKind::Inside);
        icons::at(p, rect.left_center() + vec2(18.0, 0.0), 15.0, Icon::Unlock, RED);
        p.galley(rect.left_center() + vec2(32.0, -galley.size().y / 2.0), galley, RED);
        resp
    } else {
        widgets::pill(ui, Some(Icon::Lock), "Permissions", MUTED, false)
    };
    if resp.on_hover_text(tip).clicked() {
        app.apply_settings(Settings { bypass: !on, ..app.settings.clone() });
    }
}

/// Provider switch, exact model of that provider, and effort. All three are user choices, never the router's.
fn engine_picker(app: &mut App, ui: &mut Ui) {
    let mode = |e: Engine| app.settings.auth_mode(e);
    let mut engine = app.settings.engine;
    let (resp, _) = widgets::dropdown(ui, Some(Icon::Bolt), &engine_short(engine, mode(engine)), 200.0, |ui| {
        for e in Engine::ALL {
            if widgets::menu_item(ui, e == engine, &engine_short(e, mode(e))).clicked() {
                engine = e;
            }
        }
    });
    resp.on_hover_text("Fournisseur qui exécute (System Two). Jev choisit toujours skill, outils et playbook.");
    if engine != app.settings.engine {
        let mut settings = app.settings.clone();
        settings.engine = engine;
        app.apply_settings(settings);
        app.refresh_models(engine);
        if !app.env.llm.configured() {
            app.settings_draft = Some(app.settings.clone());
        }
    }
    model_picker(app, ui);
    effort_picker(app, ui);
}

/// Exact model id sent to the provider: the account list when reachable, the catalogue otherwise.
fn model_picker(app: &mut App, ui: &mut Ui) {
    let engine = app.settings.engine;
    let current = app.settings.model(engine).to_owned();
    let fetched = app.catalog.get(&engine).map_or(&[][..], Vec::as_slice);
    let choices = models::choices(engine, &current, fetched);
    let tip = match app.catalog_error.get(&engine) {
        Some(e) => format!("Modèle envoyé à l'API : {current}\nListe du compte indisponible : {e}"),
        None => format!("Modèle envoyé à l'API : {current}\n{} modèle(s) connu(s) pour ce compte", fetched.len()),
    };
    let mut picked = current.clone();
    let mut custom = false;
    let (resp, _) = widgets::dropdown(ui, None, &models::name(engine, &current), 190.0, |ui| {
        for choice in &choices {
            if widgets::menu_item(ui, choice.id == picked, &choice.label()).clicked() {
                picked.clone_from(&choice.id);
            }
        }
        ui.separator();
        if widgets::menu_item(ui, app.model_input.is_some(), "Autre identifiant…").clicked() {
            custom = true;
        }
    });
    resp.on_hover_text(tip);
    if custom {
        app.model_input = Some(current.clone());
    }
    if picked != current && !picked.trim().is_empty() {
        pick_model(app, &picked);
    }
    if widgets::icon_button(ui, Icon::Refresh, "Récupérer la liste des modèles de ce compte").clicked() {
        app.refresh_models(engine);
    }
}

/// Free-form id for a model the provider list does not show yet.
fn custom_model(app: &mut App, ui: &mut Ui) {
    let Some(mut input) = app.model_input.take() else { return };
    let mut keep = true;
    ui.horizontal(|ui| {
        let resp = ui.add(
            TextEdit::singleline(&mut input)
                .desired_width(280.0)
                .font(FontId::proportional(12.5))
                .margin(Margin::symmetric(10, 6))
                .hint_text("identifiant exact, ex. claude-opus-5-5"),
        );
        let submit = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if widgets::pill(ui, Some(Icon::Check), "OK", ACCENT_HI, false).clicked() || submit {
            let id = input.trim().to_owned();
            if !id.is_empty() {
                pick_model(app, &id);
            }
            keep = false;
        }
        if widgets::icon_button(ui, Icon::Close, "Annuler").clicked() {
            keep = false;
        }
    });
    if keep {
        app.model_input = Some(input);
    }
}

fn pick_model(app: &mut App, id: &str) {
    app.model_input = None;
    let mut settings = app.settings.clone();
    settings.set_model(settings.engine, id);
    app.apply_settings(settings);
}

fn effort_picker(app: &mut App, ui: &mut Ui) {
    let mut effort = app.settings.effort.get(app.settings.engine);
    let label = format!("effort {}", effort.label());
    let (resp, _) = widgets::dropdown(ui, None, &label, 140.0, |ui| {
        for e in Effort::ALL {
            if widgets::menu_item(ui, e == effort, &effort_label(e)).clicked() {
                effort = e;
            }
        }
    });
    resp.on_hover_text("Budget de réflexion du moteur actif. Off = réponse directe, sans réflexion.");
    if effort != app.settings.effort.get(app.settings.engine) {
        let mut settings = app.settings.clone();
        settings.effort.set(settings.engine, effort);
        app.apply_settings(settings);
    }
}

/// Provider and how it is paid, short enough for a pill.
pub(super) fn engine_short(engine: Engine, mode: AuthMode) -> String {
    let name = engine.label().split_whitespace().next().unwrap_or_default();
    let tag = match mode {
        AuthMode::Subscription if engine.subscription_capable() => "abonnement",
        _ => "clé API",
    };
    format!("{name} · {tag}")
}

pub(super) fn effort_label(effort: Effort) -> String {
    if effort == Effort::Off { "effort Off (sans réflexion)".to_owned() } else { format!("effort {}", effort.label()) }
}

/// Round send (↑) / stop (■) button: gradient orb, glow when ready, red pulse while running.
fn action_button(ui: &mut Ui, rect: Rect, busy: bool, ready: bool, t: f64) -> bool {
    let resp = ui.interact(rect, Id::unique("composer-send"), Sense::click());
    let k = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.15);
    let on = ui.ctx().animate_bool_with_time(resp.id.with("ready"), ready || busy, 0.2);
    let rect = if resp.is_pointer_button_down_on() { rect.shrink(1.5) } else { rect };
    let p = ui.painter();
    let r = rect.width() / 2.0;
    if busy {
        fx::glow(p, rect, r, RED.gamma_multiply(0.22 + 0.16 * fx::pulse(t, 1.4)), 18);
        fx::gradient_rect(p, rect, r, fx::mix(RED, Color32::WHITE, 0.1 * k), Color32::from_rgb(0xE1, 0x1D, 0x48));
    } else {
        if on > 0.0 {
            fx::glow(p, rect, r, ACCENT.gamma_multiply((0.28 + 0.2 * k) * on), (16.0 + 10.0 * k) as u8);
        }
        p.circle_filled(rect.center(), r, theme::rgba(255, 255, 255, 14));
        if on > 0.0 {
            let shape = RectShape::filled(rect, r, Color32::WHITE);
            let grad = fx::linear(
                rect,
                fx::mix(ACCENT, ACCENT_HI, 0.3 * k).gamma_multiply(on),
                INDIGO.gamma_multiply(on),
                vec2(1.0, 1.0),
            );
            fx::paint_with(p, shape, grad);
        }
    }
    p.add(fx::edge_light(rect, r, 70.0 * on.max(0.4), 10.0));
    if busy {
        icons::at(p, rect.center(), 16.0, Icon::Stop, Color32::WHITE);
    } else {
        icons::at(p, rect.center(), 18.0, Icon::ArrowUp, fx::mix(MUTED, Color32::WHITE, on));
    }
    resp.on_hover_text(if busy { "Arrêter (Échap)" } else { "Envoyer (Entrée)" }).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_show_engine_mode_and_effort() {
        assert_eq!(engine_short(Engine::DeepSeek, AuthMode::ApiKey), "DeepSeek · clé API");
        assert_eq!(engine_short(Engine::Claude, AuthMode::Subscription), "Claude · abonnement");
        assert_eq!(engine_short(Engine::DeepSeek, AuthMode::Subscription), "DeepSeek · clé API");
        assert_eq!(effort_label(Effort::Off), "effort Off (sans réflexion)");
        assert_eq!(effort_label(Effort::Max), "effort Max");
    }
}
