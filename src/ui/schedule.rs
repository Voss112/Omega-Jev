//! Modale « Programmer un prompt » : date, heure, moteur, effort. Exécution toujours en bypass.

use chrono::{Duration, Local};
use eframe::egui::{
    self, Align, ComboBox, CornerRadius, Frame, Id, Layout, Margin, Modal, RichText, ScrollArea, TextEdit, Ui,
};

use super::{
    App,
    composer::{effort_label, engine_label},
    theme::{self, AMBER, CARD, FAINT, MUTED, RED, TEXT},
    widgets,
};
use crate::domain::{
    engine::{Effort, Engine},
    schedule::{Job, parse_when},
};

/// Form state, kept in `App` while the modal is open.
pub struct Draft {
    pub prompt: String,
    pub date: String,
    pub time: String,
    pub daily: bool,
    pub engine: Engine,
    pub effort: Effort,
}

impl Draft {
    pub fn new(prompt: String, engine: Engine, effort: Effort) -> Self {
        let tomorrow = Local::now() + Duration::days(1);
        Self {
            prompt,
            date: tomorrow.format("%Y-%m-%d").to_string(),
            time: "08:30".to_owned(),
            daily: false,
            engine,
            effort,
        }
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(mut draft) = app.schedule_draft.take() else { return };
    let (mut plan, mut cancel, mut remove) = (false, false, false);
    let modal = Modal::new(Id::unique("schedule"))
        .frame(Frame::new().fill(CARD).corner_radius(CornerRadius::same(18)).inner_margin(Margin::same(24)))
        .show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new("Programmer un prompt").font(theme::bold(20.0)).color(TEXT));
            ui.label(
                RichText::new("Omega envoie le prompt à l'heure dite, avec le moteur et l'effort choisis, toujours en mode Bypass.")
                    .size(12.0)
                    .color(MUTED),
            );
            ui.add_space(10.0);
            ui.label(RichText::new("Prompt").size(12.0).color(MUTED));
            ui.add(
                TextEdit::multiline(&mut draft.prompt)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Ce que Omega doit faire à l'heure prévue…"),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                field(ui, "Jour", |ui| {
                    ui.add(TextEdit::singleline(&mut draft.date).desired_width(110.0).hint_text("AAAA-MM-JJ"));
                });
                field(ui, "Heure", |ui| {
                    ui.add(TextEdit::singleline(&mut draft.time).desired_width(70.0).hint_text("HH:MM"));
                });
                ui.add_space(6.0);
                ui.checkbox(&mut draft.daily, "chaque jour");
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ComboBox::from_id_salt("schedule-engine")
                    .selected_text(engine_label(draft.engine, app.settings.auth_mode(draft.engine)))
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for e in Engine::ALL {
                            ui.selectable_value(&mut draft.engine, e, engine_label(e, app.settings.auth_mode(e)));
                        }
                    });
                ComboBox::from_id_salt("schedule-effort")
                    .selected_text(effort_label(draft.effort))
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for e in Effort::ALL {
                            ui.selectable_value(&mut draft.effort, e, effort_label(e));
                        }
                    });
            });
            ui.add_space(6.0);
            widgets::chip(ui, "🔓 Bypass forcé", AMBER);
            ui.add_space(10.0);
            if !app.jobs.is_empty() {
                widgets::section(ui, "Programmés");
                ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                    for job in &app.jobs {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(job.label()).size(12.0).color(TEXT));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if widgets::ghost(ui, "✕").on_hover_text("Supprimer").clicked() {
                                    remove = true;
                                }
                            });
                        });
                        ui.label(RichText::new(short(&job.prompt)).size(11.0).color(FAINT));
                    }
                });
            }
            if let Some(e) = app.schedule_error.as_deref() {
                ui.label(RichText::new(e).size(12.0).color(RED));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary(ui, "Programmer", 140.0).clicked() {
                        plan = true;
                    }
                    if widgets::ghost(ui, "Annuler").clicked() {
                        cancel = true;
                    }
                });
            });
        });
    if modal.should_close() {
        cancel = true;
    }
    if remove {
        app.jobs.clear();
        app.persist_jobs();
    }
    if plan {
        let result = parse_when(&draft.date, &draft.time);
        match result {
            Ok(_) if draft.prompt.trim().is_empty() => app.schedule_error = Some("prompt vide".to_owned()),
            Ok(at) => {
                app.schedule_error = None;
                app.jobs.push(Job::new(draft.prompt.trim().to_owned(), at, draft.daily, draft.engine, draft.effort));
                app.persist_jobs();
                app.notify("Prompt programmé — Omega doit rester ouvert");
                cancel = true;
            }
            Err(e) => app.schedule_error = Some(e),
        }
    }
    if !cancel {
        app.schedule_draft = Some(draft);
    }
}

fn field(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).size(11.0).color(FAINT));
        add(ui);
    });
    ui.add_space(10.0);
}

fn short(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or_default();
    let mut s: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        s.push('…');
    }
    s
}
