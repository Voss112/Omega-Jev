//! Right panel: subscription status and plan quotas of every connected engine (Claude, Gemini).

use std::time::{Duration, Instant};

use eframe::egui::{Align2, Context, FontId, Sense, Ui, vec2};

use super::{
    App,
    icons::{self, Icon},
    theme::{AMBER, CYAN, EDGE, FAINT, GREEN, MUTED, RED, TEXT},
    widgets,
};
use crate::domain::auth::{Provider, Window};

/// Quota refresh period of signed-in subscriptions.
const REFRESH: Duration = Duration::from_secs(30);

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
            ui.label(widgets::text(format!("relevé {}", ago(at.elapsed().as_secs())), 10.5, FAINT));
        }
    });
}

impl App {
    /// Re-reads the quotas of every signed-in subscription every `REFRESH` (one request in flight per provider).
    pub(super) fn poll_plan_usage(&mut self, ctx: &Context) {
        let due: Vec<Provider> = Provider::ENGINES
            .into_iter()
            .filter(|p| self.accounts.contains_key(p))
            .filter(|p| self.usage_polled.get(p).is_none_or(|at| at.elapsed() >= REFRESH))
            .collect();
        for provider in due {
            self.usage_polled.insert(provider, Instant::now());
            self.fetch_plan_usage(provider);
        }
        if !self.accounts.is_empty() {
            ctx.request_repaint_after(REFRESH);
        }
    }
}

/// "à l'instant", "il y a 12 s", "il y a 3 min", "il y a 2 h".
fn ago(secs: u64) -> String {
    match secs {
        0..5 => "à l'instant".to_owned(),
        5..60 => format!("il y a {secs} s"),
        60..3600 => format!("il y a {} min", secs / 60),
        _ => format!("il y a {} h", secs / 3600),
    }
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

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn age_reads_in_the_right_unit() {
        assert_eq!(ago(2), "à l'instant");
        assert_eq!(ago(42), "il y a 42 s");
        assert_eq!(ago(150), "il y a 2 min");
        assert_eq!(ago(5778), "il y a 1 h");
    }
}
