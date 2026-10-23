//! Settings modal. Only the two API keys are required; everything else has tuned defaults.

use eframe::egui::{
    self, Align, ComboBox, CornerRadius, DragValue, Frame, Grid, Id, Layout, Margin, Modal, RichText, ScrollArea, TextEdit, Ui,
};

use super::{
    App,
    theme::{self, CARD, FAINT, GREEN, MUTED, TEXT},
    widgets,
};
use crate::{
    domain::{
        auth::{AuthMode, Provider},
        settings::{Settings, Style},
    },
    infra::{auth::Auth, paths},
};

enum Action {
    Save,
    Cancel,
}

/// What the subscription section asks the app to do.
enum SubAction {
    Connect(Provider),
    Import,
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(draft) = app.settings_draft.as_mut() else { return };
    let mut action = None;
    let mut sub = None;
    let modal = Modal::new(Id::unique("settings"))
        .frame(Frame::new().fill(CARD).corner_radius(CornerRadius::same(18)).inner_margin(Margin::same(24)))
        .show(ctx, |ui| {
            ui.set_width(600.0);
            ui.label(RichText::new("Paramètres").font(theme::bold(22.0)).color(TEXT));
            ui.label(
                RichText::new("Connectez un abonnement ou saisissez une clé API. Tout le reste est réglé de façon optimale.")
                    .color(MUTED),
            );
            ui.add_space(12.0);
            ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.55).show(ui, |ui| {
                sub = form(ui, draft);
            });
            ui.add_space(10.0);
            ui.label(RichText::new(format!("Données locales : {}", paths::root().display())).size(11.5).color(FAINT));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary(ui, "Enregistrer", 130.0).clicked() {
                        action = Some(Action::Save);
                    }
                    if widgets::ghost(ui, "Annuler").clicked() {
                        action = Some(Action::Cancel);
                    }
                    if widgets::ghost(ui, "📂 Ouvrir le dossier").clicked() {
                        let _ = open::that(paths::root());
                    }
                });
            });
        });
    if modal.should_close() && action.is_none() {
        action = Some(Action::Cancel);
    }
    match action {
        Some(Action::Save) => {
            if let Some(s) = app.settings_draft.take() {
                app.apply_settings(s);
            }
        }
        Some(Action::Cancel) => app.settings_draft = None,
        None => {}
    }
    match sub {
        Some(SubAction::Connect(provider)) => app.connect(provider),
        Some(SubAction::Import) => app.import_claude(),
        None => {}
    }
}

fn form(ui: &mut Ui, s: &mut Settings) -> Option<SubAction> {
    let mut sub = None;
    section(ui, "Abonnements");
    ui.label(
        RichText::new(
            "Connexion par abonnement : la page du fournisseur s'ouvre, vous validez sur le site, Omega reçoit le jeton.",
        )
        .size(12.0)
        .color(MUTED),
    );
    for provider in Provider::ALL {
        ui.add_space(6.0);
        grid(ui, provider.id(), |ui| {
            ui.label(label(provider.label()));
            ui.horizontal(|ui| {
                let account = Auth::load(provider).map(|a| a.account()).unwrap_or_default();
                if account.is_empty() {
                    ui.label(RichText::new("Non connecté").size(12.5).color(FAINT));
                } else {
                    ui.label(RichText::new(format!("✅ Connecté · {account}")).size(12.5).color(GREEN));
                }
                if widgets::ghost(ui, "Se connecter").clicked() {
                    sub = Some(SubAction::Connect(provider));
                }
                if provider == Provider::Claude && widgets::ghost(ui, "Importer la session Claude Code").clicked() {
                    sub = Some(SubAction::Import);
                }
            });
            ui.end_row();
            ui.label(label("Mode"));
            let mut mode = match provider {
                Provider::Claude => s.claude_auth,
                Provider::Gemini => s.gemini_auth,
            };
            ComboBox::from_id_salt(("auth", provider.id()))
                .selected_text(match mode {
                    AuthMode::Subscription => "Abonnement",
                    AuthMode::ApiKey => "Clé API",
                })
                .width(200.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut mode, AuthMode::Subscription, "Abonnement");
                    ui.selectable_value(&mut mode, AuthMode::ApiKey, "Clé API");
                });
            match provider {
                Provider::Claude => s.claude_auth = mode,
                Provider::Gemini => s.gemini_auth = mode,
            }
            ui.end_row();
        });
    }
    section(ui, "Clés API");
    grid(ui, "keys", |ui| {
        secret(ui, "Kimi (Moonshot)", &mut s.kimi_key, "sk-…  (platform.kimi.ai)");
        secret(ui, "DeepSeek", &mut s.deepseek_key, "sk-…  (platform.deepseek.com)");
        secret(ui, "Claude (API)", &mut s.claude_key, "sk-ant-… (platform.claude.com) — inutile en abonnement");
        secret(ui, "Gemini (API)", &mut s.gemini_key, "AIza… (aistudio.google.com) — inutile en abonnement");
        secret(ui, "Jev (TypeSafe)", &mut s.jev_key, "console.typesafe.ai — requis pour le routage Jev");
    });
    section(ui, "Modèles");
    grid(ui, "models", |ui| {
        text(ui, "URL Kimi", &mut s.kimi_url, "");
        text(ui, "Modèle Kimi", &mut s.kimi_model, "kimi-k3.1-pro");
        text(ui, "URL DeepSeek", &mut s.deepseek_url, "");
        text(ui, "Modèle DeepSeek", &mut s.deepseek_model, "deepseek-flash = V4.1 Flash 1017");
        text(ui, "URL Claude", &mut s.claude_url, "https://api.anthropic.com (ignorée en abonnement)");
        text(ui, "Modèle Claude", &mut s.claude_model, "claude-sonnet-4-5");
        text(ui, "URL Gemini", &mut s.gemini_url, "endpoint OpenAI-compatible des clés API");
        text(ui, "Modèle Gemini", &mut s.gemini_model, "gemini-2.5-pro");
        text(ui, "URL Jev", &mut s.jev_url, "");
        text(ui, "Modèle Jev", &mut s.jev_model, "jev-latest ou version épinglée");
    });
    section(ui, "Comportement");
    grid(ui, "behavior", |ui| {
        ui.label(label("Style de sortie"));
        ComboBox::from_id_salt("style").selected_text(s.style.label()).width(220.0).show_ui(ui, |ui| {
            for style in Style::ALL {
                ui.selectable_value(&mut s.style, style, style.label());
            }
        });
        ui.end_row();
        ui.label(label("Étapes max / tour"));
        ui.add(DragValue::new(&mut s.max_steps).range(4..=96));
        ui.end_row();
        ui.label(label("Commandes sûres"));
        ui.checkbox(&mut s.auto_approve, "Exécuter sans demander (les risquées demandent toujours)");
        ui.end_row();
        ui.label(label("Bypass"));
        ui.checkbox(&mut s.bypass, "Tout accepter sans demander (commandes destructrices toujours bloquées)");
        ui.end_row();
        ui.label(label("Modifications"));
        ui.checkbox(&mut s.confirm_edits, "Valider chaque modification de fichier (diff affiché)");
        ui.end_row();
        ui.label(label("Diagnostics"));
        ui.checkbox(&mut s.auto_diagnostics, "Vérifier le code après modification (cargo check, tsc, go vet…)");
        ui.end_row();
        ui.label(label("Compaction"));
        let mut k = s.compact_at / 1000;
        ui.add(DragValue::new(&mut k).range(20..=900).suffix(" k tokens"))
            .on_hover_text("Au-delà, l'ancien historique est résumé automatiquement");
        s.compact_at = k * 1000;
        ui.end_row();
    });
    section(ui, "Confidentialité");
    grid(ui, "privacy", |ui| {
        text(ui, "Proxy", &mut s.proxy, "socks5h://127.0.0.1:9050 (Tor) — vide = direct");
        ui.label(label("Kill-switch VPN"));
        ui.checkbox(&mut s.require_vpn, "Bloquer l'envoi si aucun VPN n'est détecté");
        ui.end_row();
        ui.label(label("IP publique"));
        ui.checkbox(&mut s.ip_lookup, "Vérifier l'IP de sortie (ipwho.is)");
        ui.end_row();
    });
    section(ui, "Dossier de travail");
    ui.horizontal(|ui| {
        let current = s
            .workspace
            .as_ref()
            .map_or_else(|| format!("{} (défaut)", paths::workspace().display()), |p| p.display().to_string());
        ui.label(RichText::new(current).size(12.5).color(MUTED));
        if ui.button("Choisir…").clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            s.workspace = Some(dir);
        }
        if ui.button("Défaut").clicked() {
            s.workspace = None;
        }
    });
    sub
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(10.0);
    widgets::section(ui, title);
    ui.add_space(4.0);
}

fn grid(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    Grid::new(id).num_columns(2).spacing([16.0, 10.0]).min_col_width(140.0).show(ui, add);
}

fn label(text: &str) -> RichText {
    RichText::new(text).size(13.0).color(TEXT)
}

fn text(ui: &mut Ui, name: &str, value: &mut String, hint: &str) {
    ui.label(label(name));
    ui.add(TextEdit::singleline(value).hint_text(hint).desired_width(400.0));
    ui.end_row();
}

fn secret(ui: &mut Ui, name: &str, value: &mut String, hint: &str) {
    ui.label(label(name));
    let shown_id = Id::unique(("show-secret", name));
    let mut shown = ui.data(|d| d.get_temp::<bool>(shown_id).unwrap_or(false));
    ui.horizontal(|ui| {
        ui.add(TextEdit::singleline(value).hint_text(hint).password(!shown).desired_width(360.0));
        if ui.button(if shown { "🙈" } else { "👁" }).clicked() {
            shown = !shown;
        }
    });
    ui.data_mut(|d| d.insert_temp(shown_id, shown));
    ui.end_row();
}
