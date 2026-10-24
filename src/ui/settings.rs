//! Settings modal. Only the two API keys are required; everything else has tuned defaults.

use std::collections::HashMap;

use eframe::egui::{
    self, Align, DragValue, FontId, Grid, Id, Layout, Margin, Modal, RichText, ScrollArea, Sense, TextEdit, Ui, vec2,
};

use super::{
    App, fx,
    icons::{self, Icon},
    theme::{EDGE, EDGE_HI, FAINT, GREEN, MUTED, SOFT, TEXT},
    widgets,
};
use crate::{
    domain::{
        auth::{AuthMode, Provider},
        settings::{Settings, Style},
    },
    infra::paths,
};

const FIELD: f32 = 380.0;

enum Action {
    Save,
    Cancel,
}

/// What the subscription section asks the app to do.
enum SubAction {
    Connect(Provider),
    Import,
    UseToken,
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(draft) = app.settings_draft.as_mut() else { return };
    let mut action = None;
    let mut sub = None;
    let mut bench = false;
    let modal =
        Modal::new(Id::unique("settings")).frame(widgets::modal_frame()).backdrop_color(widgets::BACKDROP).show(ctx, |ui| {
            ui.set_width(640.0);
            if widgets::modal_header(
                ui,
                Icon::Gear,
                "Paramètres",
                "Connectez un abonnement ou saisissez une clé API. Tout le reste est réglé de façon optimale.",
            ) {
                action = Some(Action::Cancel);
            }
            ui.add_space(6.0);
            ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.6).show(ui, |ui| {
                ui.set_width(ui.available_width() - 8.0);
                sub = form(ui, draft, &mut app.token_draft, &app.accounts);
                bench = super::savings::section(ui, &app.convs, app.bench.as_deref(), app.bench_running);
                ui.add_space(4.0);
            });
            ui.add_space(14.0);
            let rect = ui.max_rect();
            fx::hairline(ui.painter(), rect.left(), rect.right(), ui.cursor().top(), EDGE_HI);
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let (icon, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                icons::paint(ui.painter(), icon, Icon::Folder, FAINT);
                ui.allocate_ui(vec2((ui.available_width() - 410.0).max(80.0), 30.0), |ui| {
                    widgets::truncated(ui, &paths::root().display().to_string(), FontId::proportional(11.5), FAINT)
                        .on_hover_text("Données locales");
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::primary_icon(ui, Some(Icon::Check), "Enregistrer", 140.0).clicked() {
                        action = Some(Action::Save);
                    }
                    if widgets::ghost(ui, "Annuler").clicked() {
                        action = Some(Action::Cancel);
                    }
                    if widgets::pill(ui, Some(Icon::Folder), "Ouvrir le dossier", MUTED, false).clicked() {
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
    if bench {
        app.run_bench();
    }
    match sub {
        Some(SubAction::Connect(provider)) => {
            // The provider page replaces this form: keep what was typed in it instead of discarding it.
            if let Some(draft) = app.settings_draft.take().filter(|d| *d != app.settings) {
                app.apply_settings(draft);
            }
            app.connect(provider);
        }
        Some(SubAction::Import) => app.import_claude(),
        Some(SubAction::UseToken) => app.use_token(),
        None => {}
    }
}

fn form(ui: &mut Ui, s: &mut Settings, token: &mut String, accounts: &HashMap<Provider, String>) -> Option<SubAction> {
    let mut sub = None;
    widgets::form_section(ui, Icon::Star, "Abonnements");
    ui.label(
        RichText::new(
            "Connexion par abonnement : la page du fournisseur s'ouvre, vous validez sur le site, Omega reçoit le jeton.",
        )
        .size(12.0)
        .color(MUTED),
    );
    for provider in Provider::ALL {
        ui.add_space(8.0);
        widgets::card(ui, EDGE, |ui| {
            ui.set_width(ui.available_width());
            grid(ui, provider.id(), |ui| {
                ui.label(RichText::new(provider.label()).font(super::theme::bold(14.0)).color(TEXT));
                ui.horizontal(|ui| match accounts.get(&provider) {
                    Some(account) => {
                        widgets::chip_icon(ui, Some(Icon::Check), format!("Connecté · {account}"), GREEN);
                    }
                    None => {
                        widgets::chip(ui, "Non connecté", FAINT);
                    }
                });
                ui.end_row();
                ui.label("");
                ui.horizontal_wrapped(|ui| {
                    if widgets::pill(ui, Some(Icon::Globe), "Se connecter", SOFT, false).clicked() {
                        sub = Some(SubAction::Connect(provider));
                    }
                    if provider == Provider::Claude
                        && widgets::pill(ui, Some(Icon::Download), "Importer la session Claude Code", SOFT, false).clicked()
                    {
                        sub = Some(SubAction::Import);
                    }
                });
                ui.end_row();
                if provider == Provider::Claude {
                    ui.label(label("Jeton longue durée"));
                    ui.horizontal(|ui| {
                        ui.add(
                            TextEdit::singleline(token)
                                .hint_text("sk-ant-oat… (claude setup-token) — évite l'échange OAuth limité")
                                .password(true)
                                .margin(Margin::symmetric(10, 6))
                                .desired_width(FIELD - 90.0),
                        );
                        if widgets::pill(ui, None, "Utiliser", SOFT, false).clicked() && !token.trim().is_empty() {
                            sub = Some(SubAction::UseToken);
                        }
                    });
                    ui.end_row();
                }
                ui.label(label("Mode"));
                let mut mode = match provider {
                    Provider::Claude => s.claude_auth,
                };
                let text = |m: AuthMode| match m {
                    AuthMode::Subscription => "Abonnement",
                    AuthMode::ApiKey => "Clé API",
                };
                widgets::dropdown(
                    ui,
                    Some(if mode == AuthMode::Subscription { Icon::Star } else { Icon::Key }),
                    text(mode),
                    200.0,
                    |ui| {
                        for m in [AuthMode::Subscription, AuthMode::ApiKey] {
                            if widgets::menu_item(ui, mode == m, text(m)).clicked() {
                                mode = m;
                            }
                        }
                    },
                );
                match provider {
                    Provider::Claude => s.claude_auth = mode,
                }
                ui.end_row();
            });
        });
    }
    widgets::form_section(ui, Icon::Key, "Clés API");
    card_grid(ui, "keys", |ui| {
        secret(ui, "DeepSeek", &mut s.deepseek_key, "sk-…  (platform.deepseek.com)");
        secret(ui, "Claude (API)", &mut s.claude_key, "sk-ant-… (platform.claude.com) — inutile en abonnement");
        secret(ui, "Jev (TypeSafe)", &mut s.jev_key, "console.typesafe.ai — requis pour le routage Jev");
    });
    widgets::form_section(ui, Icon::Cube, "Modèles");
    ui.label(
        RichText::new(
            "Le modèle se choisit aussi dans la zone de saisie : liste réelle du compte (⟳) et identifiant libre. \
             Ces champs restent la valeur de repli, modifiable à la main.",
        )
        .size(11.5)
        .color(MUTED),
    );
    ui.add_space(6.0);
    card_grid(ui, "models", |ui| {
        text(ui, "URL DeepSeek", &mut s.deepseek_url, "");
        text(ui, "Modèle DeepSeek", &mut s.deepseek_model, "deepseek-flash = V4.1 Flash 1017");
        text(ui, "URL Claude", &mut s.claude_url, "https://api.anthropic.com (ignorée en abonnement)");
        text(ui, "Modèle Claude", &mut s.claude_model, "claude-sonnet-4-5");
        text(ui, "URL Jev", &mut s.jev_url, "");
        text(ui, "Modèle Jev", &mut s.jev_model, "jev-latest ou version épinglée");
    });
    widgets::form_section(ui, Icon::Sparkle, "Comportement");
    card_grid(ui, "behavior", |ui| {
        ui.label(label("Style de sortie"));
        widgets::dropdown(ui, None, s.style.label(), 240.0, |ui| {
            for style in Style::ALL {
                if widgets::menu_item(ui, s.style == style, style.label()).clicked() {
                    s.style = style;
                }
            }
        });
        ui.end_row();
        ui.label(label("Étapes max / tour"));
        ui.add(DragValue::new(&mut s.max_steps).range(4..=96));
        ui.end_row();
        switch(ui, "Commandes sûres", &mut s.auto_approve, "Exécuter sans demander (les risquées demandent toujours)");
        switch(ui, "Bypass", &mut s.bypass, "Tout accepter sans demander (commandes destructrices toujours bloquées)");
        switch(ui, "Modifications", &mut s.confirm_edits, "Valider chaque modification de fichier (diff affiché)");
        switch(ui, "Diagnostics", &mut s.auto_diagnostics, "Vérifier le code après modification (cargo check, tsc, go vet…)");
        ui.label(label("Compaction"));
        let mut k = s.compact_at / 1000;
        ui.add(DragValue::new(&mut k).range(20..=900).suffix(" k tokens"))
            .on_hover_text("Au-delà, l'ancien historique est résumé automatiquement");
        s.compact_at = k * 1000;
        ui.end_row();
    });
    widgets::form_section(ui, Icon::Shield, "Confidentialité");
    card_grid(ui, "privacy", |ui| {
        text(ui, "Proxy", &mut s.proxy, "socks5h://127.0.0.1:9050 (Tor) — vide = direct");
        switch(ui, "Kill-switch VPN", &mut s.require_vpn, "Bloquer l'envoi si aucun VPN n'est détecté");
        switch(ui, "IP publique", &mut s.ip_lookup, "Vérifier l'IP de sortie (ipwho.is)");
    });
    widgets::form_section(ui, Icon::Folder, "Dossier de travail");
    widgets::card(ui, EDGE, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let current = s
                .workspace
                .as_ref()
                .map_or_else(|| format!("{} (défaut)", paths::workspace().display()), |p| p.display().to_string());
            ui.allocate_ui(vec2((ui.available_width() - 190.0).max(80.0), 30.0), |ui| {
                widgets::truncated(ui, &current, FontId::proportional(12.5), SOFT);
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::pill(ui, None, "Défaut", MUTED, false).clicked() {
                    s.workspace = None;
                }
                if widgets::pill(ui, Some(Icon::Folder), "Choisir…", SOFT, false).clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    s.workspace = Some(dir);
                }
            });
        });
    });
    sub
}

fn grid(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    Grid::new(id).num_columns(2).spacing([18.0, 10.0]).min_col_width(150.0).show(ui, add);
}

fn card_grid(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    widgets::card(ui, EDGE, |ui| {
        ui.set_width(ui.available_width());
        grid(ui, id, add);
    });
}

fn label(text: &str) -> RichText {
    RichText::new(text).size(13.0).color(SOFT)
}

fn text(ui: &mut Ui, name: &str, value: &mut String, hint: &str) {
    ui.label(label(name));
    ui.add(TextEdit::singleline(value).hint_text(hint).margin(Margin::symmetric(10, 6)).desired_width(FIELD));
    ui.end_row();
}

fn switch(ui: &mut Ui, name: &str, value: &mut bool, description: &str) {
    ui.label(label(name));
    widgets::toggle(ui, value, description);
    ui.end_row();
}

fn secret(ui: &mut Ui, name: &str, value: &mut String, hint: &str) {
    ui.label(label(name));
    let shown_id = Id::unique(("show-secret", name));
    let mut shown = ui.data(|d| d.get_temp::<bool>(shown_id).unwrap_or(false));
    ui.horizontal(|ui| {
        ui.add(
            TextEdit::singleline(value)
                .hint_text(hint)
                .password(!shown)
                .margin(Margin::symmetric(10, 6))
                .desired_width(FIELD - 36.0),
        );
        let icon = if shown { Icon::EyeOff } else { Icon::Eye };
        if widgets::icon_button(ui, icon, if shown { "Masquer" } else { "Afficher" }).clicked() {
            shown = !shown;
        }
    });
    ui.data_mut(|d| d.insert_temp(shown_id, shown));
    ui.end_row();
}
