//! Prompt box: Enter sends, Shift+Enter new line, engine badge, send/stop.

use eframe::egui::{
    Align, Align2, Color32, ComboBox, CornerRadius, FontId, Frame, Id, Key, Layout, Margin, Modifiers, Rect, RichText,
    ScrollArea, Sense, Stroke, TextEdit, Ui, vec2,
};

use super::{
    App,
    chat::column,
    theme::{ACCENT, AMBER, BORDER, FAINT, HOVER, MUTED, RAISED, RED, TEXT},
};
use crate::domain::{
    auth::AuthMode,
    engine::{Effort, Engine},
    settings::Settings,
};

pub fn show(app: &mut App, ui: &mut Ui) {
    let id = Id::unique("composer");
    let focused = ui.memory(|m| m.has_focus(id));
    let busy = app.busy();
    let mut send = focused && !busy && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
    let mut stop = false;
    let mut cancel_edit = false;
    let mut schedule = false;
    column(ui, |ui| {
        if app.editing.is_some() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(
                        "✏ Modification : les messages suivants et leurs changements de fichiers seront annulés à l'envoi.",
                    )
                    .size(12.0)
                    .color(AMBER),
                );
                cancel_edit = ui.small_button("Annuler (Échap)").clicked();
            });
            ui.add_space(4.0);
        }
        Frame::new()
            .fill(RAISED)
            .stroke(Stroke::new(1.0, if focused { ACCENT.gamma_multiply(0.7) } else { BORDER }))
            .corner_radius(CornerRadius::same(18))
            .inner_margin(Margin { left: 16, right: 10, top: 12, bottom: 8 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    let edit = TextEdit::multiline(&mut app.draft)
                        .id(id)
                        .hint_text("Demandez n'importe quoi…   Entrée : envoyer · Maj+Entrée : nouvelle ligne")
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
                ui.horizontal(|ui| {
                    engine_picker(app, ui);
                    bypass_toggle(app, ui);
                    if ui
                        .add(eframe::egui::Button::new(RichText::new("⏰ Programmer").size(12.5).color(MUTED)).frame(false))
                        .on_hover_text("Envoyer ce prompt plus tard, avec le moteur et l'effort choisis (toujours en bypass)")
                        .clicked()
                    {
                        schedule = true;
                    }
                    ui.label(RichText::new(app.settings.style.label()).size(11.5).color(FAINT))
                        .on_hover_text("Style de sortie (Paramètres)");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let clicked = action_button(ui, busy, !app.draft.trim().is_empty());
                        send |= clicked && !busy;
                        stop |= clicked && busy;
                    });
                });
            });
        ui.add_space(4.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Omega peut se tromper. Vérifiez le code critique.  ·  Ctrl+N nouvelle discussion · Échap stop")
                    .size(11.0)
                    .color(FAINT),
            );
        });
    });
    if cancel_edit {
        app.cancel_edit();
    }
    if schedule {
        app.open_schedule();
    }
    if send {
        app.send();
    }
    if stop && let Some(id) = app.current {
        app.stop(id);
    }
}

/// Bypass permissions: one click, clearly red while active.
fn bypass_toggle(app: &mut App, ui: &mut Ui) {
    let on = app.settings.bypass;
    let text = RichText::new(if on { "🔓 Bypass" } else { "🔒 Permissions" }).size(12.5).color(if on { RED } else { MUTED });
    let tip = if on {
        "Bypass actif : commandes, installations et modifications sont acceptées sans demander (les commandes destructrices restent bloquées). Cliquer pour revenir aux demandes."
    } else {
        "Cliquer pour accepter automatiquement toutes les autorisations (mode Bypass)."
    };
    if ui.button(text).on_hover_text(tip).clicked() {
        app.apply_settings(Settings { bypass: !on, ..app.settings.clone() });
    }
}

/// Engine switch plus the effort of the active engine. Both are user choices, never the router's.
fn engine_picker(app: &mut App, ui: &mut Ui) {
    let mode = |e: Engine| app.settings.auth_mode(e);
    let mut engine = app.settings.engine;
    ComboBox::from_id_salt("engine")
        .selected_text(RichText::new(engine_label(engine, mode(engine))).size(12.5).color(MUTED))
        .width(220.0)
        .show_ui(ui, |ui| {
            for e in Engine::ALL {
                ui.selectable_value(&mut engine, e, engine_label(e, mode(e)));
            }
        })
        .response
        .on_hover_text("Moteur qui exécute (System Two). Jev choisit toujours skill, outils et playbook.");
    if engine != app.settings.engine {
        app.apply_settings(Settings { engine, ..app.settings.clone() });
        if !app.env.llm.configured() {
            app.settings_draft = Some(app.settings.clone());
        }
    }

    let mut effort = app.settings.effort.get(app.settings.engine);
    ComboBox::from_id_salt("effort")
        .selected_text(RichText::new(effort_label(effort)).size(12.5).color(MUTED))
        .width(180.0)
        .show_ui(ui, |ui| {
            for e in Effort::ALL {
                ui.selectable_value(&mut effort, e, effort_label(e));
            }
        })
        .response
        .on_hover_text("Budget de réflexion du moteur actif. Off = réponse directe, sans réflexion.");
    if effort != app.settings.effort.get(app.settings.engine) {
        let mut settings = app.settings.clone();
        settings.effort.set(settings.engine, effort);
        app.apply_settings(settings);
    }
}

pub(super) fn engine_label(engine: Engine, mode: AuthMode) -> String {
    let tag = match mode {
        AuthMode::Subscription if engine.subscription_capable() => "abonnement",
        _ => "clé API",
    };
    format!("⚡ {} · {tag}", engine.label())
}

pub(super) fn effort_label(effort: Effort) -> String {
    if effort == Effort::Off {
        "effort Off (sans réflexion)".to_owned()
    } else {
        format!("effort {}", effort.label())
    }
}

/// Round send (↑) / stop (■) button.
fn action_button(ui: &mut Ui, busy: bool, ready: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::click());
    let p = ui.painter();
    let color = match (busy, ready, resp.hovered()) {
        (true, _, _) => RED,
        (false, true, true) => ACCENT.gamma_multiply(1.2),
        (false, true, false) => ACCENT,
        (false, false, _) => HOVER,
    };
    p.circle_filled(rect.center(), 18.0, color);
    if busy {
        p.rect_filled(Rect::from_center_size(rect.center(), vec2(11.0, 11.0)), CornerRadius::same(2), Color32::WHITE);
    } else {
        p.text(rect.center(), Align2::CENTER_CENTER, "↑", FontId::proportional(19.0), if ready { Color32::WHITE } else { MUTED });
    }
    resp.on_hover_text(if busy { "Arrêter (Échap)" } else { "Envoyer (Entrée)" }).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_show_engine_mode_and_effort() {
        assert_eq!(engine_label(Engine::Kimi, AuthMode::ApiKey), "⚡ Kimi K3.1 Pro · clé API");
        assert_eq!(engine_label(Engine::Claude, AuthMode::Subscription), "⚡ Claude · abonnement");
        assert_eq!(engine_label(Engine::Kimi, AuthMode::Subscription), "⚡ Kimi K3.1 Pro · clé API");
        assert_eq!(effort_label(Effort::Off), "effort Off (sans réflexion)");
        assert_eq!(effort_label(Effort::Max), "effort Max");
    }
}
