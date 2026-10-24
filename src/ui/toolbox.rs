//! Right panel "Outils de l'agent": a checkbox per tool; unchecked tools are never offered to the model.

use eframe::egui::Ui;

use super::{App, i18n::tr, theme::FAINT, widgets};
use crate::domain::tool::ToolKind;

fn label(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::ListDir => tr("Lister les dossiers", "List folders"),
        ToolKind::ReadFile => tr("Lire les fichiers", "Read files"),
        ToolKind::Outline => tr("Plan du code", "Code outline"),
        ToolKind::FindSymbol => tr("Trouver un symbole", "Find symbol"),
        ToolKind::Grep => tr("Recherche texte", "Text search"),
        ToolKind::BlastRadius => tr("Impact d'un changement", "Change impact"),
        ToolKind::SecurityScan => tr("Scan de sécurité", "Security scan"),
        ToolKind::EditFile => tr("Modifier un fichier", "Edit file"),
        ToolKind::ReplaceSymbol => tr("Remplacer un symbole", "Replace symbol"),
        ToolKind::WriteFile => tr("Créer un fichier", "Write file"),
        ToolKind::Shell => tr("Commandes shell", "Shell commands"),
        ToolKind::Computer => tr("Souris et clavier", "Mouse & keyboard"),
        _ => kind.name(),
    }
}

pub fn show(app: &mut App, ui: &mut Ui, heading: fn(&mut Ui, &str)) {
    heading(ui, tr("Outils de l'agent", "Agent tools"));
    let mut changed = None;
    for kind in ToolKind::ALL {
        let mut on = !app.settings.disabled_tools.contains(&kind);
        if ui.checkbox(&mut on, label(kind)).on_hover_text(kind.name()).changed() {
            changed = Some((kind, on));
        }
    }
    ui.label(widgets::text(tr("Décochés : jamais proposés au modèle.", "Unchecked: never offered to the model."), 10.5, FAINT));
    if let Some((kind, on)) = changed {
        let mut settings = app.settings.clone();
        if on {
            settings.disabled_tools.remove(&kind);
        } else {
            settings.disabled_tools.insert(kind);
        }
        app.apply_settings(settings);
    }
}
