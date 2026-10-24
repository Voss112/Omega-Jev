//! Modale « Programmer un prompt » : date, heure, moteur, effort. Exécution toujours en bypass.

use chrono::{Duration, Local};
use eframe::egui::{self, Align, FontId, Id, Layout, Margin, Modal, RichText, ScrollArea, Sense, TextEdit, Ui, vec2};

use super::{
    App,
    composer::{effort_label, engine_short},
    icons::{self, Icon},
    theme::{AMBER, EDGE, FAINT, MUTED, RED, TEXT},
    widgets,
};
use crate::domain::{
    engine::{Effort, Engine},
    models,
    schedule::{Job, parse_when},
};

/// Form state, kept in `App` while the modal is open.
pub struct Draft {
    pub prompt: String,
    pub date: String,
    pub time: String,
    pub daily: bool,
    pub engine: Engine,
    pub model: String,
    pub effort: Effort,
    /// Job being modified from the Tasks tab (its run history is kept).
    pub editing: Option<uuid::Uuid>,
}

impl Draft {
    /// The form filled with an existing job, to modify it.
    pub fn of(job: &Job) -> Self {
        let local = job.at.with_timezone(&Local);
        Self {
            prompt: job.prompt.clone(),
            date: local.format("%Y-%m-%d").to_string(),
            time: local.format("%H:%M").to_string(),
            daily: job.daily,
            engine: job.engine,
            model: job.model.clone(),
            effort: job.effort,
            editing: Some(job.id),
        }
    }

    pub fn new(prompt: String, engine: Engine, model: String, effort: Effort) -> Self {
        let tomorrow = Local::now() + Duration::days(1);
        Self {
            prompt,
            date: tomorrow.format("%Y-%m-%d").to_string(),
            time: "08:30".to_owned(),
            daily: false,
            engine,
            model,
            effort,
            editing: None,
        }
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(mut draft) = app.schedule_draft.take() else { return };
    let (mut plan, mut cancel, mut remove) = (false, false, None);
    let modal =
        Modal::new(Id::unique("schedule")).frame(widgets::modal_frame()).backdrop_color(widgets::BACKDROP).show(ctx, |ui| {
            ui.set_width(580.0);
            cancel |= widgets::modal_header(
                ui,
                Icon::Clock,
                "Programmer un prompt",
                "Omega envoie le prompt à l'heure dite, avec le moteur et l'effort choisis, toujours en mode Bypass.",
            );
            ui.add_space(16.0);
            field_label(ui, "Prompt");
            ui.add(
                TextEdit::multiline(&mut draft.prompt)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(12, 10))
                    .hint_text("Ce que Omega doit faire à l'heure prévue…"),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                field(ui, "Jour", |ui| {
                    ui.add(
                        TextEdit::singleline(&mut draft.date)
                            .desired_width(120.0)
                            .margin(Margin::symmetric(10, 6))
                            .hint_text("AAAA-MM-JJ"),
                    );
                });
                field(ui, "Heure", |ui| {
                    ui.add(
                        TextEdit::singleline(&mut draft.time)
                            .desired_width(80.0)
                            .margin(Margin::symmetric(10, 6))
                            .hint_text("HH:MM"),
                    );
                });
                field(ui, "Répétition", |ui| {
                    widgets::toggle(ui, &mut draft.daily, "chaque jour");
                });
            });
            ui.add_space(12.0);
            field_label(ui, "Moteur");
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                let previous = draft.engine;
                widgets::dropdown(
                    ui,
                    Some(Icon::Bolt),
                    &engine_short(draft.engine, app.settings.auth_mode(draft.engine)),
                    220.0,
                    |ui| {
                        for e in Engine::ALL {
                            if widgets::menu_item(ui, e == draft.engine, &engine_short(e, app.settings.auth_mode(e))).clicked() {
                                draft.engine = e;
                            }
                        }
                    },
                );
                if draft.engine != previous {
                    draft.model = app.settings.model(draft.engine).to_owned();
                }
                let fetched = app.catalog.get(&draft.engine).map_or(&[][..], Vec::as_slice);
                let choices = models::choices(draft.engine, &draft.model, fetched);
                widgets::dropdown(ui, None, &models::name(draft.engine, &draft.model), 200.0, |ui| {
                    for choice in &choices {
                        if widgets::menu_item(ui, choice.id == draft.model, &choice.label()).clicked() {
                            draft.model.clone_from(&choice.id);
                        }
                    }
                });
                widgets::dropdown(ui, None, &effort_label(draft.effort), 180.0, |ui| {
                    for e in Effort::ALL {
                        if widgets::menu_item(ui, e == draft.effort, &effort_label(e)).clicked() {
                            draft.effort = e;
                        }
                    }
                });
            });
            ui.add_space(8.0);
            widgets::chip_icon(ui, Some(Icon::Unlock), "Bypass forcé", AMBER);
            if app.jobs.iter().any(Job::pending) {
                widgets::form_section(ui, Icon::Clock, "Programmés");
                ScrollArea::vertical().max_height(170.0).show(ui, |ui| {
                    for (i, job) in app.jobs.iter().enumerate().filter(|(_, j)| j.pending()) {
                        widgets::card(ui, EDGE, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.set_width(ui.available_width() - 40.0);
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.label(RichText::new(job.label()).size(12.5).color(TEXT));
                                    widgets::truncated(ui, &short(&job.prompt), FontId::proportional(11.5), FAINT);
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if widgets::icon_button(ui, Icon::Trash, "Supprimer").clicked() {
                                        remove = Some(i);
                                    }
                                });
                            });
                        });
                        ui.add_space(4.0);
                    }
                });
            }
            if let Some(e) = app.schedule_error.as_deref() {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(15.0, 15.0), Sense::hover());
                    icons::paint(ui.painter(), rect, Icon::Alert, RED);
                    ui.label(RichText::new(e).size(12.0).color(RED));
                });
            }
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary_icon(ui, Some(Icon::Clock), "Programmer", 150.0).clicked() {
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
    if let Some(i) = remove.filter(|i| *i < app.jobs.len()) {
        app.jobs.remove(i);
        app.persist_jobs();
    }
    let mut planned = false;
    if plan {
        let result = parse_when(&draft.date, &draft.time);
        match result {
            Ok(_) if draft.prompt.trim().is_empty() => app.schedule_error = Some("prompt vide".to_owned()),
            Ok(at) => {
                app.schedule_error = None;
                let mut job =
                    Job::new(draft.prompt.trim().to_owned(), at, draft.daily, draft.engine, draft.model.clone(), draft.effort);
                match draft.editing.and_then(|id| app.jobs.iter_mut().find(|j| j.id == id)) {
                    Some(old) => {
                        // Same task, new schedule: id and history stay, a finished one-shot becomes pending again.
                        job.id = old.id;
                        job.runs = std::mem::take(&mut old.runs);
                        *old = job;
                        app.notify("Tâche modifiée");
                    }
                    None => {
                        app.jobs.push(job);
                        app.notify("Prompt programmé — Omega doit rester ouvert");
                    }
                }
                app.persist_jobs();
                planned = true;
            }
            Err(e) => app.schedule_error = Some(e),
        }
    }
    if planned {
        return;
    }
    if cancel {
        // The prompt came from the composer: closing without scheduling gives it back.
        if draft.editing.is_none() && app.draft.trim().is_empty() {
            app.draft = draft.prompt;
        }
    } else {
        app.schedule_draft = Some(draft);
    }
}

fn field_label(ui: &mut Ui, label: &str) {
    ui.label(RichText::new(label).size(11.5).color(MUTED));
    ui.add_space(2.0);
}

fn field(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        field_label(ui, label);
        add(ui);
    });
    ui.add_space(14.0);
}

fn short(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or_default();
    let mut s: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        s.push('…');
    }
    s
}
