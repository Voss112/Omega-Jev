//! Right panel: live tool health (green/red), network privacy, session economy, last Jev verdict.

use eframe::egui::{Align, Color32, Layout, ProgressBar, RichText, ScrollArea, Spinner, Ui};

use super::{
    App,
    theme::{self, ACCENT, AMBER, BORDER, CYAN, FAINT, GREEN, MUTED, RED, SOFT, TEXT},
    widgets,
};
use crate::app::health;

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.add_space(18.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Outils").font(theme::bold(17.0)).color(TEXT));
        let ok = app.health.iter().filter(|h| h.ok).count();
        if !app.health.is_empty() {
            let color = if ok == app.health.len() { GREEN } else { AMBER };
            widgets::chip(ui, format!("{ok}/{} actifs", app.health.len()), color);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::ghost(ui, "⟳").on_hover_text("Relancer les auto-tests").clicked() {
                app.refresh_health();
            }
        });
    });
    ui.add_space(8.0);
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        tools(app, ui);
        network(app, ui);
        economy(app, ui);
        subscription(app, ui);
        verdict(app, ui);
        ui.add_space(16.0);
    });
}

fn row(ui: &mut Ui, ok: bool, name: &str, detail: &str) {
    ui.horizontal(|ui| {
        widgets::dot(ui, ok);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new(name).size(13.0).color(if ok { TEXT } else { SOFT }));
            let short: String = detail.chars().take(44).collect();
            ui.label(RichText::new(short).size(11.0).color(if ok { FAINT } else { RED.gamma_multiply(0.8) }))
                .on_hover_text(detail);
        });
    });
    ui.add_space(2.0);
}

fn tools(app: &mut App, ui: &mut Ui) {
    if app.health.is_empty() {
        ui.horizontal(|ui| {
            ui.add(Spinner::new().size(14.0).color(ACCENT));
            ui.label(RichText::new("Auto-tests en cours…").color(MUTED));
        });
        return;
    }
    let mut group = "";
    let mut create_rules = false;
    for h in &app.health {
        if h.group != group {
            ui.add_space(8.0);
            widgets::section(ui, h.group);
            ui.add_space(2.0);
            group = h.group;
        }
        row(ui, h.ok, h.name, &h.detail);
        if h.name == health::RULES_CHECK && !h.ok {
            create_rules = ui.small_button("➕ Créer OMEGA.md (modèle)").clicked();
        }
    }
    if create_rules {
        app.create_rules_file();
    }
}

fn network(app: &App, ui: &mut Ui) {
    ui.add_space(8.0);
    widgets::section(ui, "Réseau & anonymat");
    ui.add_space(2.0);
    match &app.vpn {
        Some(v) => {
            let detail = [v.tunnel.as_deref(), v.country.as_deref(), v.org.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            row(ui, v.active, "VPN", if detail.is_empty() { "aucun tunnel détecté" } else { &detail });
        }
        None => row(ui, false, "VPN", "détection…"),
    }
    let proxy = app.settings.proxy.trim();
    row(ui, !proxy.is_empty(), "Proxy / Tor", if proxy.is_empty() { "connexion directe" } else { proxy });
    row(ui, true, "Télémétrie", "aucune · données 100 % locales");
    row(
        ui,
        !app.settings.bypass,
        "Autorisations",
        if app.settings.bypass { "BYPASS : tout est accepté sans demander" } else { "demandées pour les actions risquées" },
    );
    row(
        ui,
        app.settings.require_vpn,
        "Kill-switch VPN",
        if app.settings.require_vpn { "envoi bloqué sans VPN" } else { "désactivé" },
    );
}

fn economy(app: &App, ui: &mut Ui) {
    let Some(conv) = app.current_conv() else { return };
    let u = conv.usage;
    ui.add_space(10.0);
    widgets::section(ui, "Économie de la session");
    ui.add_space(4.0);
    widgets::card(ui, BORDER, |ui| {
        ui.set_width(ui.available_width());
        let line = |ui: &mut Ui, k: &str, v: String| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(k).size(12.5).color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(v).size(12.5).monospace().color(TEXT))
                });
            });
        };
        line(ui, "Entrée", widgets::human(u.prompt));
        line(ui, "Sortie", widgets::human(u.completion));
        line(ui, "Raisonnement", widgets::human(u.reasoning));
        line(ui, "Jev (contrôle)", widgets::human(u.jev));
        line(ui, "Outils chargés", conv.toolset.len().to_string());
        line(ui, "Contexte actuel", format!("{} / {}", widgets::human(conv.context), widgets::human(app.settings.compact_at)));
        if let Some(s) = &conv.summary {
            line(ui, "Résumé actif", format!("{} msg condensés", s.upto));
        }
        ui.add_space(4.0);
        ui.label(RichText::new(format!("Cache KV : {:.0} % de l'entrée", u.cache_ratio() * 100.0)).size(12.0).color(CYAN));
        ui.add(ProgressBar::new(u.cache_ratio()).desired_height(6.0).fill(CYAN));
    });
}

fn subscription(app: &App, ui: &mut Ui) {
    if !app.settings.engine.subscription_capable() {
        return;
    }
    ui.add_space(10.0);
    widgets::section(ui, "Abonnement");
    ui.add_space(4.0);
    widgets::card(ui, BORDER, |ui| {
        ui.set_width(ui.available_width());
        let connected = app.env.llm.subscription();
        let account = app.env.llm.account();
        let label = if connected {
            let suffix = if account.is_empty() { String::new() } else { format!(" · {account}") };
            format!("✅ {} connecté{suffix}", app.settings.engine.label())
        } else {
            format!("{} : non connecté (⚙ Paramètres)", app.settings.engine.label())
        };
        ui.label(RichText::new(label).size(12.5).color(if connected { GREEN } else { MUTED }));
        let usage = app.plan_usage.as_ref().filter(|(p, _, _)| p.engine() == app.settings.engine);
        if let Some((_, windows, at)) = usage {
            ui.add_space(4.0);
            for w in windows {
                ui.horizontal(|ui| {
                    ui.add_sized([58.0, 16.0], eframe::egui::Label::new(RichText::new(&w.label).size(11.5).color(MUTED)));
                    ui.add(
                        ProgressBar::new(w.fraction)
                            .desired_height(6.0)
                            .fill(CYAN)
                            .text(RichText::new(format!("{:.0} %", w.fraction * 100.0)).size(10.5)),
                    );
                });
            }
            ui.label(RichText::new(format!("relevé il y a {} s", at.elapsed().as_secs())).size(10.5).color(FAINT));
        }
    });
}

fn verdict(app: &App, ui: &mut Ui) {
    ui.add_space(10.0);
    widgets::section(ui, "Dernier verdict Jev");
    ui.add_space(4.0);
    let Some(route) = app.current_conv().and_then(|c| c.messages.iter().rev().find_map(|m| m.route.as_ref())) else {
        widgets::card(ui, BORDER, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Aucun routage pour l'instant").size(12.5).color(SOFT));
            ui.label(RichText::new("Jev se prononce au premier message envoyé.").size(11.0).color(FAINT));
        });
        return;
    };
    widgets::card(ui, BORDER, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(route.skill.label()).font(theme::bold(15.0)).color(TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (label, color) = match route.source {
                    crate::domain::route::Source::Jev => ("⚡ Jev", CYAN),
                    crate::domain::route::Source::Local => ("◌ local", MUTED),
                };
                widgets::chip(ui, label, color);
            });
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("confiance").size(11.0).color(FAINT));
            ui.label(RichText::new(format!("{:.0} %", route.confidence * 100.0)).size(11.5).monospace().color(TEXT));
            if route.risky() {
                widgets::chip(ui, "🛡 revue sécurité", RED);
            }
        });
        ui.add_space(8.0);
        let tools = app.current_conv().map_or(0, |c| c.toolset.len());
        ui.columns(2, |cols| {
            let left = &mut cols[0];
            stat(left, "Code", route.code, ACCENT);
            left.add_space(6.0);
            stat(left, "Risque", route.risk, RED);
            left.add_space(6.0);
            stat(left, "Apps", route.apps, GREEN);
            let right = &mut cols[1];
            stat(right, "Web", route.web, CYAN);
            right.add_space(6.0);
            stat(right, "Impact", route.blast, AMBER);
            right.add_space(6.0);
            stat_count(right, "Outils", tools);
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if let Some(p) = &route.playbook {
                widgets::chip(ui, format!("📘 {p}"), GREEN);
            }
            if route.needs_web() {
                widgets::chip(ui, "🌐 web", CYAN);
            }
            if route.needs_apps() {
                widgets::chip(ui, "🧩 apps", GREEN);
            }
        });
    });
}

/// Label, value and a 3 px bar — readable in a 292 px panel.
fn stat(ui: &mut Ui, label: &str, value: f32, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(10.5).color(FAINT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let text = if value > 0.0 { format!("{:.0} %", value * 100.0) } else { "—".to_owned() };
            ui.label(RichText::new(text).size(11.0).monospace().color(if value > 0.0 { color } else { FAINT }));
        });
    });
    bar(ui, value, color);
}

fn stat_count(ui: &mut Ui, label: &str, count: usize) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(10.5).color(FAINT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(count.to_string()).size(11.0).monospace().color(TEXT));
        });
    });
    bar(ui, (count as f32 / 8.0).min(1.0), MUTED);
}

fn bar(ui: &mut Ui, value: f32, color: Color32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(eframe::egui::vec2(width, 3.0), eframe::egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, BORDER);
    if value > 0.0 {
        let filled = eframe::egui::Rect::from_min_size(rect.min, eframe::egui::vec2(width * value.clamp(0.0, 1.0), 3.0));
        p.rect_filled(filled, 2.0, color);
    }
}
