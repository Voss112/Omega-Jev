//! Composer extras: joined text files (picker, drop, chips) and the live engine/model/effort of a running turn.

use std::path::PathBuf;

use eframe::egui::{Context, Ui};

use super::{
    App,
    i18n::tr, attach,
    composer::{effort_label, engine_short},
    docs::{self, MAX_DOCS},
    icons::Icon,
    theme::{CYAN, FAINT, SOFT},
    widgets,
};
use crate::domain::models;

impl App {
    /// Picked files: images become attachments, text files are inlined with the prompt.
    pub(super) fn import_files(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            if attach::is_image(&path) {
                match attach::from_path(&path) {
                    Ok(img) => self.attach(img),
                    Err(e) => self.notify(e),
                }
            } else {
                let doc = docs::from_path(&path);
                self.add_doc(doc);
            }
        }
    }

    /// Non-image files dropped this frame (images go through `collect_images`).
    pub(super) fn collect_docs(&mut self, ctx: &Context) {
        for doc in docs::dropped(ctx, attach::is_image) {
            self.add_doc(doc);
        }
    }

    fn add_doc(&mut self, doc: Result<docs::Doc, String>) {
        match doc {
            Err(e) => self.notify(e),
            Ok(_) if self.docs.len() >= MAX_DOCS => self.notify(format!("{MAX_DOCS} {}", tr("fichiers max par message", "files max per message"))),
            Ok(d) if self.docs.contains(&d) => {}
            Ok(d) => self.docs.push(d),
        }
    }
}

/// Removable chips for the joined files.
pub fn doc_chips(app: &mut App, ui: &mut Ui) {
    if app.docs.is_empty() {
        return;
    }
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for (i, doc) in app.docs.iter().enumerate() {
            let label = format!("{} · {} {}", doc.name, doc.text.lines().count(), tr("lignes", "lines"));
            widgets::chip_icon(ui, Some(Icon::File), label, SOFT);
            if widgets::icon_button(ui, Icon::Close, tr("Retirer ce fichier", "Remove this file")).clicked() {
                remove = Some(i);
            }
        }
    });
    if let Some(i) = remove {
        app.docs.remove(i);
    }
    ui.add_space(6.0);
}

/// Greyed read-only pills: what the running turn uses right now (auto tier, delegation, Jev effort).
pub fn live_pills(app: &App, ui: &mut Ui) {
    let Some(live) = app.current.and_then(|id| app.live.get(&id)) else { return };
    let mode = app.settings.auth_mode(live.engine);
    let tip = tr(
        "Choisi automatiquement pour cette tâche (modèle auto, délégation, effort Jev) — modifiable une fois le tour terminé",
        "Picked automatically for this task (auto model, delegation, Jev effort) — editable once the turn ends",
    );
    let changed = live.model != app.settings.model(live.engine) || live.engine != app.settings.engine;
    let color = |moved: bool| if moved { CYAN } else { FAINT };
    widgets::chip_icon(ui, Some(Icon::Bolt), engine_short(live.engine, mode), color(live.engine != app.settings.engine))
        .on_hover_text(tip);
    widgets::chip(ui, models::name(live.engine, &live.model), color(changed)).on_hover_text(format!("{tip}\n{}", live.model));
    let effort = live.effort.map_or_else(|| "effort …".to_owned(), effort_label);
    let lowered = live.effort.is_some_and(|e| e != app.settings.effort.get(live.engine));
    widgets::chip(ui, effort, color(lowered)).on_hover_text(tip);
}
