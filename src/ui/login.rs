//! OAuth modal: open the provider page, validate on the site, come back.

use eframe::egui::{self, Align, CornerRadius, Frame, Id, Layout, Margin, Modal, RichText, TextEdit, Ui};

use super::{
    App,
    theme::{self, CARD, FAINT, MUTED, TEXT},
    widgets,
};
use crate::{app::login::Login, domain::auth::Provider};

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(mut login) = app.login.take() else { return };
    let (mut submit, mut cancel, mut open_page) = (false, false, false);
    let modal = Modal::new(Id::unique("login"))
        .frame(Frame::new().fill(CARD).corner_radius(CornerRadius::same(18)).inner_margin(Margin::same(24)))
        .show(ctx, |ui| {
            ui.set_width(580.0);
            ui.label(RichText::new(format!("Se connecter avec {}", login.provider.label())).font(theme::bold(20.0)).color(TEXT));
            ui.add_space(8.0);
            instructions(ui, &login);
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("🌐 Ouvrir la page").clicked() {
                    open_page = true;
                }
                if ui.button("⧉ Copier le lien").clicked() {
                    ui.ctx().copy_text(login.url.clone());
                }
            });
            if login.provider == Provider::Claude {
                ui.add_space(10.0);
                ui.label(RichText::new("Collez le code affiché après validation").size(12.5).color(MUTED));
                ui.add(TextEdit::singleline(&mut login.paste).hint_text("code#state").desired_width(f32::INFINITY));
                ui.add_space(6.0);
                submit = widgets::primary(ui, "Valider", 130.0).clicked() && !login.paste.trim().is_empty();
            }
            if !login.status.is_empty() {
                ui.add_space(10.0);
                ui.label(RichText::new(login.status.clone()).size(12.0).color(FAINT));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::ghost(ui, "Annuler").clicked() {
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
        login.status = "Validation…".to_owned();
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
    let text = match (login.provider, login.listening) {
        (Provider::Claude, _) => {
            "1. Ouvrez la page et connectez-vous avec votre compte Claude (Pro ou Max). \
             2. Après validation, la page affiche un code : copiez-le et collez-le ci-dessous."
        }
        (Provider::Gemini, true) => {
            "1. Ouvrez la page et choisissez le compte Google de votre abonnement. \
             2. Après validation, Omega reçoit le retour du navigateur automatiquement."
        }
        (Provider::Gemini, false) => {
            "Ouvrez la page, validez, puis collez ici le code affiché si le navigateur ne revient pas tout seul."
        }
    };
    ui.label(RichText::new(text).size(12.5).color(MUTED));
}
