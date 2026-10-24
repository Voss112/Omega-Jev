//! Tasks tab: every scheduled prompt — pending, paused, finished — with its runs and the conversations they opened.

use std::time::{Duration, Instant};

use chrono::{Local, Utc};
use eframe::egui::{Align, Layout, RichText, ScrollArea, Ui};
use uuid::Uuid;

use super::{
    App,
    i18n::{english, tr},
    chat::column,
    icons::Icon,
    theme::{self, ACCENT_HI, AMBER, CYAN, EDGE, FAINT, GREEN, MUTED, RED, SOFT, TEXT},
    widgets,
};
use crate::domain::schedule::Job;

const DELETE_CONFIRM: Duration = Duration::from_secs(3);

/// Which jobs a section of the tab lists.
type Filter = fn(&Job) -> bool;

enum Action {
    Run(Uuid),
    Toggle(Uuid),
    Edit(Uuid),
    Delete(Uuid),
    Open(Uuid),
    New,
    Close,
}

pub fn view(app: &mut App, ui: &mut Ui) {
    let mut action = None;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.set_height(34.0);
        ui.label(RichText::new(tr("Tâches programmées", "Scheduled tasks")).font(theme::bold(17.0)).color(TEXT));
        let pending = app.jobs.iter().filter(|j| j.pending() && !j.paused).count();
        widgets::chip(ui, format!("{pending} {}", tr("active(s)", "active")), if pending > 0 { CYAN } else { FAINT });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill(ui, Some(Icon::Close), tr("Fermer", "Close"), MUTED, false).clicked() {
                action = Some(Action::Close);
            }
            if widgets::pill(ui, Some(Icon::Plus), tr("Programmer…", "Schedule…"), ACCENT_HI, false).clicked() {
                action = Some(Action::New);
            }
        });
    });
    ui.label(widgets::text(tr("Exécutées en mode Bypass, avec leur propre moteur — Omega doit rester ouvert.", "Run in Bypass mode with their own engine — Omega must stay open."), 12.0, FAINT));
    ui.add_space(6.0);
    let armed = app.task_delete_armed.filter(|(_, at)| at.elapsed() < DELETE_CONFIRM).map(|(id, _)| id);
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        column(ui, |ui| {
            if app.jobs.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(widgets::text(tr("Aucune tâche. « Programmer… » ou le bouton ⏰ de la zone de saisie.", "No tasks. “Schedule…” or the ⏰ button of the input box."), 13.0, FAINT));
                });
                return;
            }
            let groups: [(&str, Filter); 3] = [
                (tr("À venir", "Upcoming"), |j| j.pending() && !j.paused),
                (tr("En pause", "Paused"), |j| j.pending() && j.paused),
                (tr("Terminées", "Done"), |j| !j.pending()),
            ];
            for (title, keep) in groups {
                let jobs: Vec<&Job> = app.jobs.iter().filter(|j| keep(j)).collect();
                if jobs.is_empty() {
                    continue;
                }
                widgets::form_section(ui, Icon::Clock, title);
                for job in jobs {
                    let running = job.runs.last().is_some_and(|r| app.live.contains_key(&r.conv));
                    if let Some(clicked) = card(ui, job, running, armed == Some(job.id)) {
                        action = Some(clicked);
                    }
                    ui.add_space(6.0);
                }
            }
        });
    });
    match action {
        Some(Action::Run(id)) => app.run_job_now(id),
        Some(Action::Toggle(id)) => {
            if let Some(j) = app.jobs.iter_mut().find(|j| j.id == id) {
                j.paused = !j.paused;
                // A paused daily job resumes at its next occurrence, not with a burst of missed runs.
                if !j.paused && j.daily {
                    j.advance(Utc::now());
                }
            }
            app.persist_jobs();
        }
        Some(Action::Edit(id)) => {
            if let Some(job) = app.jobs.iter().find(|j| j.id == id) {
                app.schedule_draft = Some(super::schedule::Draft::of(job));
                app.schedule_error = None;
            }
        }
        Some(Action::Delete(id)) if armed == Some(id) => {
            app.task_delete_armed = None;
            app.jobs.retain(|j| j.id != id);
            app.persist_jobs();
        }
        Some(Action::Delete(id)) => app.task_delete_armed = Some((id, Instant::now())),
        Some(Action::Open(conv)) => {
            app.show_tasks = false;
            app.select(conv);
        }
        Some(Action::New) => app.open_schedule(),
        Some(Action::Close) => app.show_tasks = false,
        None => {}
    }
}

fn card(ui: &mut Ui, job: &Job, running: bool, armed: bool) -> Option<Action> {
    let mut action = None;
    let stroke = if running { CYAN.gamma_multiply(0.5) } else { EDGE };
    widgets::card(ui, stroke, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (label, color) = match (running, job.paused, job.done) {
                (true, _, _) => (tr("en cours", "running"), CYAN),
                (_, true, _) => (tr("en pause", "paused"), AMBER),
                (_, _, true) => (tr("terminée", "done"), GREEN),
                _ => (tr("programmée", "scheduled"), ACCENT_HI),
            };
            widgets::chip(ui, label, color);
            ui.label(RichText::new(job.label()).size(12.5).color(SOFT));
        });
        ui.add_space(4.0);
        ui.label(RichText::new(job.prompt.clone()).size(13.0).color(TEXT));
        if job.pending() && !job.paused {
            ui.label(widgets::text(format!("{} {}", tr("Prochaine exécution :", "Next run:"), countdown(job)), 11.5, FAINT));
        }
        if let Some(last) = job.runs.last() {
            ui.horizontal(|ui| {
                let when = last.at.with_timezone(&Local).format("%d/%m %H:%M");
                ui.label(widgets::text(if english() {
                    format!("Last run: {when} · {} in total", job.runs.len())
                } else {
                    format!("Dernière exécution : {when} · {} au total", job.runs.len())
                }, 11.5, FAINT));
                if widgets::pill(ui, Some(Icon::Bubble), tr("Ouvrir la discussion", "Open the chat"), MUTED, false).clicked() {
                    action = Some(Action::Open(last.conv));
                }
            });
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if widgets::pill(ui, Some(Icon::Bolt), tr("Lancer maintenant", "Run now"), SOFT, false).clicked() && !running {
                action = Some(Action::Run(job.id));
            }
            if job.pending() {
                let (icon, text) = if job.paused { (Icon::Refresh, tr("Reprendre", "Resume")) } else { (Icon::Stop, tr("Mettre en pause", "Pause")) };
                if widgets::pill(ui, Some(icon), text, SOFT, false).clicked() {
                    action = Some(Action::Toggle(job.id));
                }
            }
            if widgets::pill(ui, Some(Icon::Pencil), tr("Modifier", "Edit"), SOFT, false).clicked() {
                action = Some(Action::Edit(job.id));
            }
            let (text, color) = if armed { (tr("Re-cliquer pour supprimer", "Click again to delete"), RED) } else { (tr("Supprimer", "Delete"), MUTED) };
            if widgets::pill(ui, Some(Icon::Trash), text, color, armed).clicked() {
                action = Some(Action::Delete(job.id));
            }
        });
    });
    action
}

/// `dans 2 h 05` / `dans 3 j` / `maintenant`.
fn countdown(job: &Job) -> String {
    let left = job.at - Utc::now();
    let minutes = left.num_minutes();
    let at = job.at.with_timezone(&Local).format("%d/%m %H:%M");
    if english() {
        return match minutes {
            ..=0 => format!("{at} (now)"),
            1..=59 => format!("{at} (in {minutes} min)"),
            60..=2879 => format!("{at} (in {} h {:02})", minutes / 60, minutes % 60),
            _ => format!("{at} (in {} d)", minutes / 1440),
        };
    }
    match minutes {
        ..=0 => format!("{at} (maintenant)"),
        1..=59 => format!("{at} (dans {minutes} min)"),
        60..=2879 => format!("{at} (dans {} h {:02})", minutes / 60, minutes % 60),
        _ => format!("{at} (dans {} j)", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::engine::{Effort, Engine};

    #[test]
    fn countdowns_read_naturally() {
        let job = |mins: i64| Job::new("x".into(), Utc::now() + chrono::Duration::minutes(mins), false, Engine::Claude, String::new(), Effort::Low);
        assert!(countdown(&job(-5)).contains("maintenant"));
        assert!(countdown(&job(30)).contains("min"));
        assert!(countdown(&job(125)).contains(" h "));
        assert!(countdown(&job(4 * 1440)).contains(" j)"));
    }
}
