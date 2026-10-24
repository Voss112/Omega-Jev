//! Settings section: the token economy measured on every request, and the live benchmark that proves each lever.

use eframe::egui::{Grid, RichText, Ui};

use super::{
    i18n::tr,
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
    widgets::form_section(ui, Icon::Bolt, tr("Économies de tokens · preuve", "Token savings · proof"));
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
            ui.label(RichText::new(tr("Aucune requête mesurée pour l'instant : envoyez un message.", "No request measured yet: send a message.")).size(12.5).color(FAINT));
            return;
        }
        ledger_grid(ui, &ledger, jev, convs.iter().filter(|c| c.savings.requests > 0).count());
    });
    ui.add_space(8.0);
    let mut run = false;
    ui.horizontal_wrapped(|ui| {
        if running {
            ui.spinner();
            ui.label(RichText::new(tr("Banc d'essai en cours…", "Benchmark running…")).size(12.5).color(SOFT));
        } else if widgets::pill(ui, Some(Icon::Flask), tr("Lancer le banc d'essai", "Run the benchmark"), SOFT, false).clicked() {
            run = true;
        }
        ui.label(
            RichText::new(tr("Moteur et clés enregistrés · quelques milliers de tokens (comptages Claude gratuits)", "Saved engine and keys · a few thousand tokens (Claude counts are free)"))
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
                    widgets::chip_icon(ui, Some(icon), if r.ok { "OK" } else { tr("Échec", "Failed") }, color);
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
        line(tr("Requêtes mesurées", "Measured requests"), format!("{} · {convs} {}", l.requests, tr("discussion(s)", "chat(s)")));
        line(tr("Entrée envoyée", "Input sent"), format!("{} tokens", h(l.prompt)));
        line(tr("  lue du cache", "  read from cache"), format!("{} ({:.0} %)", h(l.cached), pct(l.cached as f64, l.prompt as f64)));
        line(tr("  écrite en cache", "  written to cache"), h(l.written));
        line(tr("Sortie", "Output"), format!("{} tokens", h(l.completion)));
        line(tr("Jev (contrôle)", "Jev (control)"), format!("{} tokens", h(jev)));
        line(tr("Évité · lazy tools (Jev)", "Avoided · lazy tools (Jev)"), format!("−{}", h(l.lazy)));
        line(tr("Évité · RTK (sorties shell)", "Avoided · RTK (shell output)"), format!("−{}", h(l.rtk)));
        line(tr("Évité · playbooks non renvoyés", "Avoided · playbooks not resent"), format!("−{}", h(l.playbook)));
        line(tr("Évité · compaction", "Avoided · compaction"), format!("−{}", h(l.summary)));
        line(tr("Évité · résultats effacés", "Avoided · cleared results"), format!("−{}", h(l.cleared)));
        line(tr("Remise · cache préfixe", "Discount · prefix cache"), format!("−{} {}", tr("équiv.", "equiv."), h(l.cache_saved().max(0.0) as u64)));
        line(tr("Sans Omega ni cache", "Without Omega or cache"), format!("{} {}", h(l.baseline() as u64), tr("tokens plein tarif", "tokens at full price")));
        line(tr("Sans leviers (cache gardé)", "Without levers (cache kept)"), format!("{} {}", h(l.without_levers() as u64), tr("équiv.", "equiv.")));
        line(tr("Avec Omega (facturé)", "With Omega (billed)"), format!("{} {}", h(l.billed as u64), tr("équiv.", "equiv.")));
        if jev > 0 {
            line(
                tr("Bilan Jev au tarif réel", "Jev balance at real price"),
                format!(
                    "{:.2}× ({} {} / {} tokens Jev)",
                    l.jev_billed / jev as f64,
                    h(l.jev_billed as u64),
                    tr("équiv. évités", "equiv. avoided"),
                    h(jev)
                ),
            );
        }
        let calib = if l.calibrated() {
            format!("1 token ≈ {:.2} {}", 1.0 / l.ratio(), tr("octets (mesuré)", "bytes (measured)"))
        } else {
            tr("par défaut (images seules)", "default (images only)").to_owned()
        };
        line(tr("Calibrage", "Calibration"), calib);
    });
    ui.add_space(8.0);
    let share = l.saved_share() as f32;
    ui.horizontal(|ui| {
        ui.label(RichText::new(tr("Économie mesurée sur l'entrée", "Measured input savings")).size(12.5).color(GREEN));
        ui.label(RichText::new(format!("{:.1} %", share * 100.0)).monospace().size(12.5).color(GREEN));
    });
    widgets::bar(ui, share, GREEN, 6.0);
}
