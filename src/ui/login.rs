//! OAuth modal: open the provider page, validate on the site, come back.

use eframe::egui::{self, Align, FontId, Id, Layout, Margin, Modal, RichText, Sense, TextEdit, Ui, vec2};

use super::{
    App,
    i18n::{english, tr}, fx,
    icons::{self, Icon},
    theme::{ACCENT_HI, EDGE, FAINT, MUTED, SOFT, TEXT},
    widgets,
};
use crate::{app::login::Login, domain::auth::Provider};

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(mut login) = app.login.take() else { return };
    let (mut submit, mut cancel, mut open_page) = (false, false, false);
    let modal = Modal::new(Id::unique("login")).frame(widgets::modal_frame()).backdrop_color(widgets::BACKDROP).show(ctx, |ui| {
        ui.set_width(580.0);
        let title = if english() { format!("Sign in with {}", login.provider.label()) } else { format!("Se connecter avec {}", login.provider.label()) };
        cancel |=
            widgets::modal_header(ui, Icon::Key, &title, tr("Validation sur le site du fournisseur, rien ne transite ailleurs.", "Approval happens on the provider's site, nothing goes anywhere else."));
        ui.add_space(14.0);
        instructions(ui, &login);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if widgets::primary_icon(ui, Some(Icon::Globe), tr("Ouvrir la page", "Open the page"), 170.0).clicked() {
                open_page = true;
            }
            ui.add_space(4.0);
            if widgets::pill(ui, Some(Icon::Copy), tr("Copier le lien", "Copy the link"), SOFT, false).clicked() {
                ui.ctx().copy_text(login.url.clone());
            }
        });
        if !login.user_code.is_empty() {
            ui.add_space(14.0);
            ui.label(RichText::new(tr("Code à saisir sur GitHub", "Code to enter on GitHub")).size(12.5).color(MUTED));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&login.user_code).font(FontId::monospace(26.0)).color(TEXT));
                ui.add_space(8.0);
                if widgets::pill(ui, Some(Icon::Copy), tr("Copier le code", "Copy the code"), SOFT, false).clicked() {
                    ui.ctx().copy_text(login.user_code.clone());
                }
            });
        }
        if login.accepts_paste() {
            ui.add_space(14.0);
            let prompt = match login.provider {
                Provider::Gemini => tr("Si la page de retour ne s'ouvre pas, collez son adresse complète ici", "If the return page does not open, paste its full address here"),
                _ => tr("Collez le code affiché après validation", "Paste the code shown after approval"),
            };
            ui.label(RichText::new(prompt).size(12.5).color(MUTED));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add(
                    TextEdit::singleline(&mut login.paste)
                        .hint_text(if login.provider == Provider::Gemini { "http://127.0.0.1:…/oauth2callback?…&code=…" } else { "code#state" })
                        .margin(Margin::symmetric(10, 7))
                        .desired_width(ui.available_width() - 140.0),
                );
                submit = widgets::primary_icon(ui, Some(Icon::Check), tr("Valider", "Confirm"), 130.0).clicked()
                    && !login.paste.trim().is_empty()
                    && !login.submitting;
            });
        }
        if !login.status.is_empty() {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                if login.submitting {
                    fx::spinner(ui.painter(), rect.center(), 6.0, 2.0, fx::now(ui.ctx()));
                } else {
                    icons::paint(ui.painter(), rect, Icon::Sparkle, ACCENT_HI);
                }
                ui.label(RichText::new(login.status.clone()).size(12.0).color(FAINT));
            });
        }
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::ghost(ui, tr("Annuler", "Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
    });
    if modal.should_close() {
        cancel = true;
    }
    if open_page {
        let _ = open::that(&login.url);
    }
    if submit {
        login.status = tr("Validation…", "Checking…").to_owned();
        app.login = Some(login);
        app.submit_code();
        return;
    }
    if cancel {
        app.cancel_login();
    } else {
        app.login = Some(login);
    }
}

fn instructions(ui: &mut Ui, login: &Login) {
    let steps: [&str; 2] = match login.provider {
        Provider::Claude => [
            tr("Ouvrez la page et connectez-vous avec votre compte Claude (Pro ou Max).", "Open the page and sign in with your Claude account (Pro or Max)."),
            tr("Après validation, la page affiche un code : copiez-le et collez-le ci-dessous.", "Once approved, the page shows a code: copy it and paste it below."),
        ],
        Provider::Gemini => [
            tr("La page Google s'est ouverte : connectez-vous avec le compte de votre abonnement Gemini (AI Pro ou Ultra).", "The Google page opened: sign in with your Gemini subscription account (AI Pro or Ultra)."),
            tr("Acceptez l'accès : Omega reçoit la réponse tout seul et active Gemini Code Assist sur ce compte.", "Allow access: Omega receives the answer by itself and enables Gemini Code Assist on this account."),
        ],
        Provider::GitHub => [
            tr("La page github.com/login/device s'est ouverte : saisissez le code ci-dessous.", "The github.com/login/device page opened: enter the code below."),
            tr("Autorisez l'accès : git et gh fonctionneront dans le shell de l'agent (option dans ⚙ Paramètres).", "Allow access: git and gh will work in the agent's shell (option in ⚙ Settings)."),
        ],
    };
    widgets::card(ui, EDGE, |ui| {
        ui.set_width(ui.available_width());
        for (i, step) in steps.iter().enumerate() {
            if i > 0 {
                ui.add_space(6.0);
            }
            ui.horizontal_top(|ui| {
                let (badge, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                let p = ui.painter();
                fx::gradient_rect(p, badge, 11.0, super::theme::ACCENT, super::theme::INDIGO);
                p.text(
                    badge.center(),
                    egui::Align2::CENTER_CENTER,
                    (i + 1).to_string(),
                    FontId::proportional(11.5),
                    egui::Color32::WHITE,
                );
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(*step).size(12.5).color(SOFT)).wrap());
            });
        }
    });
}
