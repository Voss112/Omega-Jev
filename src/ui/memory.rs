//! Right panel "Mémoire": import `.md` notes that every turn's system prompt then carries.

use eframe::egui::{FontId, Ui};

use super::{
    App,
    icons::Icon,
    theme::{ACCENT_HI, FAINT, SOFT},
    widgets,
};
use crate::infra::memory::{self, MAX_FILES, Note};

enum Action {
    Import,
    Remove(String),
}

pub fn show(app: &mut App, ui: &mut Ui, heading: fn(&mut Ui, &str)) {
    heading(ui, "Mémoire");
    let notes = app.memory_notes.get_or_insert_with(|| memory::list(&memory::dir())).clone();
    let mut action = None;
    if notes.is_empty() {
        ui.label(widgets::text("Aucune note : importez des .md (préférences, contexte) lus à chaque requête.", 11.5, FAINT));
    }
    for note in &notes {
        if row(ui, note) {
            action = Some(Action::Remove(note.name.clone()));
        }
    }
    ui.add_space(4.0);
    let tip = format!("Ajouter des fichiers .md à la mémoire ({MAX_FILES} max, {} car. envoyés au plus)", memory::MAX_CHARS);
    if widgets::pill(ui, Some(Icon::Plus), "Importer des .md", ACCENT_HI, false).on_hover_text(tip).clicked() {
        action = Some(Action::Import);
    }
    match action {
        Some(Action::Import) => app.import_memory(),
        Some(Action::Remove(name)) => app.forget_memory(&name),
        None => return,
    }
    app.memory_notes = None;
}

/// One note; `true` when its remove button was clicked.
fn row(ui: &mut Ui, note: &Note) -> bool {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 34.0).max(60.0);
        ui.allocate_ui(eframe::egui::vec2(width, 20.0), |ui| {
            let label = format!("{} · {:.1} Ko", note.name, note.bytes as f32 / 1024.0);
            widgets::truncated(ui, &label, FontId::proportional(12.5), SOFT);
        });
        widgets::icon_button(ui, Icon::Trash, "Retirer de la mémoire").clicked()
    })
    .inner
}

impl App {
    fn import_memory(&mut self) {
        let picked = rfd::FileDialog::new().add_filter("Markdown", &["md"]).pick_files().unwrap_or_default();
        let dir = memory::dir();
        for path in picked {
            match memory::add(&dir, &path) {
                Ok(name) => self.notify(format!("{name} ajouté à la mémoire")),
                Err(e) => self.notify(format!("{e:#}")),
            }
        }
    }

    fn forget_memory(&mut self, name: &str) {
        match memory::remove(&memory::dir(), name) {
            Ok(()) => self.notify(format!("{name} retiré de la mémoire")),
            Err(e) => self.notify(format!("{e:#}")),
        }
    }
}
