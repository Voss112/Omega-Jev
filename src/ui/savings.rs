//! Settings section: the token economy measured on every request, and the live benchmark that proves each lever.

use eframe::egui::{Grid, RichText, Ui};

use super::{
    icons::Icon,
    theme::{EDGE, FAINT, GREEN, MUTED, RED, SOFT, TEXT},
    widgets,
};
use crate::{
    app::health::Health,
    domain::{chat::Conversation, savings::Ledger},
};

/// `true` when the user asks for a benchmark run.
pub fn section(ui: &mut Ui, convs: &[Conversation], bench: Option<&[Health]>, running: bool) -> bool {
    let mut ledger = Ledger::default();
    let mut jev = 0;
    for c in convs {
        ledger += c.savings;
        jev += c.usage.jev;
    }
    widgets::form_section(ui, Icon::Bolt, "Économies de tokens · preuve");
    ui.label(
        RichText::new(
            "Chiffres du fournisseur (exacts) pour chaque requête envoyée, toutes discussions confondues. \
             Les leviers sont mesurés en octets sur la vraie requête puis convertis avec le ratio tokens/octet \
             calibré sur vos propres requêtes.",
        )
        .size(11.5)
        .color(MUTED),
    );
    ui.add_space(6.0);
    widgets::card(ui, EDGE, |ui| {
        ui.set_width(ui.available_width());
        if ledger.requests == 0 {
            ui.label(RichText::new("Aucune requête mesurée pour l'instant : envoyez un message.").size(12.5).color(FAINT));
            return;
        }
        ledger_grid(ui, &ledger, jev, convs.iter().filter(|c| c.savings.requests > 0).count());
    });
    ui.add_space(8.0);
    let mut run = false;
    ui.horizontal_wrapped(|ui| {
        if running {
            ui.spinner();
            ui.label(RichText::new("Banc d'essai en cours…").size(12.5).color(SOFT));
        } else if widgets::pill(ui, Some(Icon::Flask), "Lancer le banc d'essai", SOFT, false).clicked() {
            run = true;
        }
        ui.label(
            RichText::new("Moteur et clés enregistrés · quelques milliers de tokens (comptages Claude gratuits)")
                .size(11.0)
                .color(FAINT),
        );
    });
    if let Some(rows) = bench {
        ui.add_space(6.0);
        widgets::card(ui, EDGE, |ui| {
            ui.set_width(ui.available_width());
            for (i, r) in rows.iter().enumerate() {
                if i > 0 {
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    let (icon, color) = if r.ok { (Icon::Check, GREEN) } else { (Icon::Alert, RED) };
                    widgets::chip_icon(ui, Some(icon), if r.ok { "OK" } else { "Échec" }, color);
                    ui.label(RichText::new(r.name).font(super::theme::bold(13.0)).color(TEXT));
                });
                ui.label(RichText::new(&r.detail).size(12.0).color(if r.ok { SOFT } else { RED }));
            }
        });
    }
    run
}

fn ledger_grid(ui: &mut Ui, l: &Ledger, jev: u64, convs: usize) {
    let h = widgets::human;
    let pct = |part: f64, whole: f64| if whole > 0.0 { part * 100.0 / whole } else { 0.0 };
    Grid::new("economy").num_columns(2).spacing([18.0, 6.0]).min_col_width(190.0).show(ui, |ui| {
        let mut line = |name: &str, value: String| {
            ui.label(RichText::new(name).size(12.5).color(SOFT));
            ui.label(RichText::new(value).monospace().size(12.0).color(TEXT));
            ui.end_row();
        };
        line("Requêtes mesurées", format!("{} · {convs} discussion(s)", l.requests));
        line("Entrée envoyée", format!("{} tokens", h(l.prompt)));
        line("  lue du cache", format!("{} ({:.0} %)", h(l.cached), pct(l.cached as f64, l.prompt as f64)));
        line("  écrite en cache", h(l.written));
        line("Sortie", format!("{} tokens", h(l.completion)));
        line("Jev (contrôle)", format!("{} tokens", h(jev)));
        line("Évité · lazy tools (Jev)", format!("−{}", h(l.lazy)));
        line("Évité · RTK (sorties shell)", format!("−{}", h(l.rtk)));
        line("Évité · playbooks non renvoyés", format!("−{}", h(l.playbook)));
        line("Évité · compaction", format!("−{}", h(l.summary)));
        line("Évité · résultats effacés", format!("−{}", h(l.cleared)));
        line("Remise · cache préfixe", format!("−{} équiv.", h(l.cache_saved().max(0.0) as u64)));
        line("Sans Omega ni cache", format!("{} tokens plein tarif", h(l.baseline() as u64)));
        line("Sans leviers (cache gardé)", format!("{} équiv.", h(l.without_levers() as u64)));
        line("Avec Omega (facturé)", format!("{} équiv.", h(l.billed as u64)));
        if jev > 0 {
            line(
                "Bilan Jev au tarif réel",
                format!("{:.2}× ({} équiv. évités / {} tokens Jev)", l.jev_billed / jev as f64, h(l.jev_billed as u64), h(jev)),
            );
        }
        let calib = if l.calibrated() {
            format!("1 token ≈ {:.2} octets (mesuré)", 1.0 / l.ratio())
        } else {
            "par défaut (images seules)".to_owned()
        };
        line("Calibrage", calib);
    });
    ui.add_space(8.0);
    let share = l.saved_share() as f32;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Économie mesurée sur l'entrée").size(12.5).color(GREEN));
        ui.label(RichText::new(format!("{:.1} %", share * 100.0)).monospace().size(12.5).color(GREEN));
    });
    widgets::bar(ui, share, GREEN, 6.0);
}
