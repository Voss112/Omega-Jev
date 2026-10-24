//! Right panel "Outils de l'agent": a checkbox per tool; unchecked tools are never offered to the model.
//! Folded by default: the heading toggles the list.

use eframe::egui::{FontId, Id, Sense, Ui, pos2, vec2};

use super::{
    App, fx,
    i18n::tr,
    icons,
    theme::{self, FAINT, MUTED, SOFT, TEXT, TILE_HOVER},
    widgets,
};
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
        ToolKind::DocsSearch => tr("Documentation des librairies", "Library docs"),
        ToolKind::WebSearch => tr("Recherche web", "Web search"),
        ToolKind::WebFetch => tr("Lire une page web", "Fetch web page"),
        ToolKind::CliAnything => tr("Piloter des logiciels (CLI)", "Drive apps (CLI)"),
        ToolKind::AuditKit => tr("Kit d'audit de sécurité", "Security audit kit"),
        ToolKind::Kits => tr("Kits externes (design, web)", "External kits (design, web)"),
        ToolKind::Computer => tr("Souris et clavier", "Mouse & keyboard"),
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let open_id = Id::unique("toolbox-open");
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
    let on = ToolKind::ALL.len() - app.settings.disabled_tools.len();
    ui.add_space(14.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    let resp = resp.on_hover_text(if open { tr("Replier", "Collapse") } else { tr("Déplier la liste", "Expand the list") });
    if resp.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(open_id, open));
    }
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let flip = ui.ctx().animate_bool_with_time(open_id, open, 0.16);
    let p = ui.painter();
    if hover > 0.0 {
        p.rect_filled(rect, 7.0, TILE_HOVER.gamma_multiply(hover));
    }
    icons::chevron(p, pos2(rect.left() + 10.0, rect.center().y), 13.0, flip, fx::mix(MUTED, TEXT, hover));
    let title = tr("Outils de l'agent", "Agent tools").to_uppercase();
    let galley = p.layout_job(fx::spaced(&title, theme::bold(10.0), fx::mix(FAINT, SOFT, hover), 1.1));
    p.galley(pos2(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0), galley, FAINT);
    p.text(
        rect.right_center() - vec2(6.0, 0.0),
        eframe::egui::Align2::RIGHT_CENTER,
        format!("{on}/{}", ToolKind::ALL.len()),
        FontId::monospace(10.5),
        FAINT,
    );
    if !open {
        return;
    }
    ui.add_space(4.0);
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
