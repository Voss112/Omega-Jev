//! Settings modal. Only the two API keys are required; everything else has tuned defaults.

use std::collections::HashMap;

use eframe::egui::{
    self, Align, DragValue, FontId, Grid, Id, Layout, Margin, Modal, RichText, ScrollArea, Sense, TextEdit, Ui, vec2,
};

use super::{
    App, fx,
    i18n::tr,
    icons::{self, Icon},
    theme::{EDGE, EDGE_HI, FAINT, GREEN, MUTED, SOFT, TEXT},
    widgets,
};
use crate::{
    domain::{
        auth::{AuthMode, Provider},
        settings::{DICTATION_LANGUAGES, Settings, Style, UiLang},
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
    GitHubToken,
    Disconnect(Provider),
    InstallAgy,
    AgyLogin,
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
                sub = form(ui, draft, &mut app.token_draft, &mut app.github_draft, &app.accounts);
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
        Some(SubAction::GitHubToken) => app.use_github_token(),
        Some(SubAction::Disconnect(provider)) => app.disconnect(provider),
        Some(SubAction::InstallAgy) => app.install_agy(true),
        Some(SubAction::AgyLogin) => app.agy_login(),
        None => {}
    }
}

fn form(
    ui: &mut Ui,
    s: &mut Settings,
    token: &mut String,
    github: &mut String,
    accounts: &HashMap<Provider, String>,
) -> Option<SubAction> {
    let mut sub = None;
    widgets::form_section(ui, Icon::Star, "Abonnements");
    ui.label(
        RichText::new(
            "Connexion par abonnement : la page du fournisseur s'ouvre, vous validez sur le site, Omega reçoit le jeton.",
        )
        .size(12.0)
        .color(MUTED),
    );
    for provider in Provider::ENGINES {
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
                if provider == Provider::Gemini {
                    ui.label(label("Antigravity"));
                    ui.horizontal_wrapped(|ui| match crate::infra::llm::agy::binary() {
                        Some(_) => {
                            widgets::chip_icon(ui, Some(Icon::Check), "agy installé · abonnement Pro/Ultra (texte)", GREEN);
                            if widgets::pill(ui, Some(Icon::Globe), "Se connecter à Antigravity", SOFT, false)
                                .on_hover_text("Ouvre agy dans une console : connexion Google, puis /quit")
                                .clicked()
                            {
                                sub = Some(SubAction::AgyLogin);
                            }
                        }
                        None => {
                            if widgets::pill(ui, Some(Icon::Download), "Installer Antigravity (agy)", SOFT, false)
                                .on_hover_text("Google réserve désormais les abonnements personnels à l'Antigravity CLI")
                                .clicked()
                            {
                                sub = Some(SubAction::InstallAgy);
                            }
                        }
                    });
                    ui.end_row();
                }
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
                    Provider::Gemini => s.gemini_auth,
                    _ => s.claude_auth,
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
                    Provider::Gemini => s.gemini_auth = mode,
                    _ => s.claude_auth = mode,
                }
                ui.end_row();
            });
        });
    }
    sub = github_section(ui, s, github, accounts.get(&Provider::GitHub)).or(sub);
    widgets::form_section(ui, Icon::Key, "Clés API");
    card_grid(ui, "keys", |ui| {
        secret(ui, "DeepSeek", &mut s.deepseek_key, "sk-…  (platform.deepseek.com)");
        secret(ui, "Claude (API)", &mut s.claude_key, "sk-ant-… (platform.claude.com) — inutile en abonnement");
        secret(ui, "Gemini (API)", &mut s.gemini_key, "AIza… (aistudio.google.com) — inutile en abonnement");
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
        text(ui, "URL Gemini", &mut s.gemini_url, "endpoint OpenAI-compatible des clés API (ignorée en abonnement)");
        text(ui, "Modèle Gemini", &mut s.gemini_model, "gemini-3.8-pro");
        text(ui, "Gemini Flash", &mut s.gemini_flash_model, "gemini-3.8-flash — reçoit le travail délégué");
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
        ui.label(label("Langue de dictée"));
        let current = DICTATION_LANGUAGES.iter().find(|(code, _)| *code == s.dictation_language).map_or("Français", |(_, l)| l);
        widgets::dropdown(ui, None, current, 240.0, |ui| {
            for (code, name) in DICTATION_LANGUAGES {
                if widgets::menu_item(ui, s.dictation_language == code, name).clicked() {
                    s.dictation_language = code.into();
                }
            }
        });
        ui.end_row();
        ui.label(label(tr("Étapes max / tour", "Max steps / turn")));
        ui.horizontal(|ui| {
            ui.add_enabled(!s.unlimited_steps, DragValue::new(&mut s.max_steps).range(4..=96));
            ui.add_space(10.0);
            ui.checkbox(&mut s.unlimited_steps, tr("Infinie", "Unlimited")).on_hover_text(tr(
                "Aucune limite d'étapes : le tour continue jusqu'à la réponse finale (■ Stop pour arrêter)",
                "No step limit: the turn runs until the final answer (■ Stop to end it)",
            ));
        });
        ui.end_row();
        switch(ui, tr("100 % autonome", "Fully autonomous"), &mut s.autonomous, tr(
            "Commandes, installations, push : tout s'exécute sans demander (seules les commandes destructrices restent bloquées)",
            "Commands, installs, pushes: everything runs without asking (only destructive commands stay blocked)",
        ));
        switch(ui, tr("Souris et clavier", "Mouse & keyboard"), &mut s.computer, tr(
            "L'agent voit l'écran et pilote la souris et le clavier si besoin (moteurs avec vision)",
            "The agent sees the screen and drives mouse and keyboard when needed (vision engines)",
        ));
        switch(ui, tr("Reprise après limite", "Resume after limit"), &mut s.resume_on_quota, tr(
            "Limite Claude atteinte : surveille les % d'utilisation et reprend seul dès que le quota retombe",
            "Claude limit reached: watches the usage % and resumes by itself once the quota drops",
        ));
        switch(ui, tr("Commandes sûres", "Safe commands"), &mut s.auto_approve, tr("Exécuter sans demander (les risquées demandent toujours)", "Run without asking (risky ones always ask)"));
        switch(ui, "Bypass", &mut s.bypass, tr("Tout accepter sans demander (commandes destructrices toujours bloquées)", "Accept everything (destructive commands still blocked)"));
        switch(ui, tr("Modifications", "Edits"), &mut s.confirm_edits, tr("Valider chaque modification de fichier (diff affiché)", "Approve each file edit (diff shown)"));
        switch(ui, "Diagnostics", &mut s.auto_diagnostics, tr("Vérifier le code après modification (cargo check, tsc, go vet…)", "Check code after edits (cargo check, tsc, go vet…)"));
        switch(ui, tr("Effort auto", "Auto effort"), &mut s.auto_effort, tr("Effort ajusté par Jev (plafond = votre choix)", "Effort tuned by Jev (capped by your choice)"));
        switch(ui, tr("Délégation", "Delegation"), &mut s.delegate, tr("Questions simples et résumés confiés à un modèle Flash", "Simple questions and summaries go to a Flash model"));
        switch(ui, tr("Gemini Flash d'abord", "Gemini Flash first"), &mut s.delegate_gemini, tr("Gemini Flash reçoit la délégation ; DeepSeek Flash si indisponible ou quota à 100 %", "Gemini Flash takes delegation; DeepSeek Flash if unavailable or quota at 100 %"));
        switch(ui, tr("Tests & commentaires", "Tests & comments"), &mut s.delegate_routine, tr("Tests (unitaires, intégration, end-to-end) et commentaires de code délégués aussi", "Tests (unit, integration, end-to-end) and code comments delegated too"));
        switch(ui, tr("Modèle auto (Claude)", "Auto model (Claude)"), &mut s.auto_model, tr("Jev choisit Haiku, Sonnet ou votre modèle selon la difficulté de chaque tour", "Jev picks Haiku, Sonnet or your model per turn difficulty"));
        switch(ui, tr("Tests en continu", "Live tests"), &mut s.live_tests, tr("Tests relancés en arrière-plan pendant le code (cargo watch, vitest, ptw…) ; l'agent ne les relance plus à la fin", "Tests re-run in the background while coding (cargo watch, vitest, ptw…); the agent skips the final run"));
        ui.label(label("Compaction"));
        let mut k = s.compact_at / 1000;
        ui.add(DragValue::new(&mut k).range(20..=900).suffix(" k tokens"))
            .on_hover_text(tr("Au-delà, l'ancien historique est résumé automatiquement", "Beyond this, old history is summarized automatically"));
        s.compact_at = k * 1000;
        ui.end_row();
    });
    widgets::form_section(ui, Icon::Shield, tr("Confidentialité", "Privacy"));
    card_grid(ui, "privacy", |ui| {
        text(ui, "Proxy", &mut s.proxy, tr("socks5h://127.0.0.1:9050 (Tor) — vide = direct", "socks5h://127.0.0.1:9050 (Tor) — empty = direct"));
        switch(ui, "Kill-switch VPN", &mut s.require_vpn, tr("Bloquer l'envoi si aucun VPN n'est détecté", "Block sending when no VPN is detected"));
        switch(ui, tr("IP publique", "Public IP"), &mut s.ip_lookup, tr("Vérifier l'IP de sortie (ipwho.is)", "Check the exit IP (ipwho.is)"));
    });
    widgets::form_section(ui, Icon::Gear, tr("Système", "System"));
    card_grid(ui, "system", |ui| {
        ui.label(label(tr("Langue", "Language")));
        ui.horizontal(|ui| {
            ui.selectable_value(&mut s.ui_lang, UiLang::Fr, "Français");
            ui.selectable_value(&mut s.ui_lang, UiLang::En, "English");
        });
        ui.end_row();
        switch(ui, tr("Mises à jour", "Updates"), &mut s.auto_update, tr("Télécharger chaque release GitHub en silence, installée à la fermeture", "Download each GitHub release silently, installed on exit"));
        switch(ui, tr("Zone de notification", "System tray"), &mut s.tray, tr("Icône près de l'horloge ; fermer la fenêtre l'y range", "Icon next to the clock; closing the window hides it there"));
        switch(ui, tr("Démarrage", "Start-up"), &mut s.autostart, tr("Lancer Omega à l'ouverture de la session Windows", "Start Omega when Windows signs in"));
    });
    widgets::form_section(ui, Icon::Folder, tr("Dossier de travail", "Workspace"));
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

/// GitHub account: device-flow login or a pasted token, shared with the agent's shell when allowed.
fn github_section(ui: &mut Ui, s: &mut Settings, token: &mut String, account: Option<&String>) -> Option<SubAction> {
    let mut sub = None;
    widgets::form_section(ui, Icon::Globe, "GitHub");
    card_grid(ui, "github", |ui| {
        ui.label(RichText::new("GitHub").font(super::theme::bold(14.0)).color(TEXT));
        ui.horizontal(|ui| match account {
            Some(login) => {
                widgets::chip_icon(ui, Some(Icon::Check), format!("Connecté · {login}"), GREEN);
            }
            None => {
                widgets::chip(ui, "Non connecté", FAINT);
            }
        });
        ui.end_row();
        ui.label("");
        ui.horizontal_wrapped(|ui| {
            if widgets::pill(ui, Some(Icon::Globe), "Se connecter à GitHub", SOFT, false).clicked() {
                sub = Some(SubAction::Connect(Provider::GitHub));
            }
            if account.is_some() && widgets::pill(ui, Some(Icon::Close), "Se déconnecter", MUTED, false).clicked() {
                sub = Some(SubAction::Disconnect(Provider::GitHub));
            }
        });
        ui.end_row();
        ui.label(label("Jeton"));
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(token)
                    .hint_text("ghp_… ou github_pat_… (au lieu de la connexion)")
                    .password(true)
                    .margin(Margin::symmetric(10, 6))
                    .desired_width(FIELD - 90.0),
            );
            if widgets::pill(ui, None, "Utiliser", SOFT, false).clicked() && !token.trim().is_empty() {
                sub = Some(SubAction::GitHubToken);
            }
        });
        ui.end_row();
        switch(ui, "Shell de l'agent", &mut s.github_shell, "git push / gh authentifiés avec ce compte (jeton jamais écrit sur disque)");
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
