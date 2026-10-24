//! Right panel: subscription status and plan quotas of every connected engine (Claude, Gemini).

use eframe::egui::{Align2, FontId, Sense, Ui, vec2};

use super::{
    App,
    icons::{self, Icon},
    theme::{AMBER, CYAN, EDGE, FAINT, GREEN, MUTED, RED, TEXT},
    widgets,
};
use crate::domain::auth::{Provider, Window};

/// Providers worth a card: signed in, or the active engine when it can run on a subscription.
fn shown(app: &App) -> Vec<Provider> {
    let active = Provider::of(app.settings.engine).filter(|_| app.settings.engine.subscription_capable());
    Provider::ENGINES.into_iter().filter(|p| app.accounts.contains_key(p) || Some(*p) == active).collect()
}

pub fn show(app: &App, ui: &mut Ui, heading: impl Fn(&mut Ui, &str)) {
    let providers = shown(app);
    if providers.is_empty() {
        return;
    }
    heading(ui, "Abonnements");
    for provider in providers {
        card(app, ui, provider);
        ui.add_space(6.0);
    }
}

fn card(app: &App, ui: &mut Ui, provider: Provider) {
    widgets::card(ui, EDGE, |ui| {
        ui.set_width(ui.available_width());
        let active = Provider::of(app.settings.engine) == Some(provider) && app.env.llm.subscription();
        let account = app.accounts.get(&provider).cloned().or_else(|| active.then(|| app.env.llm.account()));
        let connected = account.is_some();
        let label = match account.as_deref() {
            Some(a) if !a.is_empty() => format!("{} connecté · {a}", provider.label()),
            Some(_) => format!("{} connecté", provider.label()),
            None => format!("{} : non connecté (Paramètres)", provider.label()),
        };
        let color = if connected { GREEN } else { MUTED };
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), rect, if connected { Icon::Check } else { Icon::Key }, color);
            widgets::truncated(ui, &label, FontId::proportional(12.5), color);
        });
        if let Some((windows, at)) = app.plan_usage.get(&provider) {
            ui.add_space(6.0);
            windows.iter().for_each(|w| quota(ui, w));
            ui.label(widgets::text(format!("relevé il y a {} s", at.elapsed().as_secs()), 10.5, FAINT));
        }
    });
}

fn quota(ui: &mut Ui, w: &Window) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    let p = ui.painter();
    p.text(rect.left_center(), Align2::LEFT_CENTER, &w.label, FontId::proportional(11.5), MUTED);
    p.text(rect.right_center(), Align2::RIGHT_CENTER, format!("{:.0} %", w.fraction * 100.0), FontId::monospace(11.0), TEXT);
    widgets::bar(ui, w.fraction, level(w.fraction), 5.0);
    ui.add_space(4.0);
}

fn level(fraction: f32) -> eframe::egui::Color32 {
    if fraction > 0.85 {
        RED
    } else if fraction > 0.6 {
        AMBER
    } else {
        CYAN
    }
}
