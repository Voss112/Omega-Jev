//! Right panel: live tool health (green/red), network privacy, session economy, last Jev verdict.

use eframe::egui::{Align, Color32, FontId, Layout, Rect, ScrollArea, Sense, Ui, pos2, vec2};

use super::{
    i18n::{english, tr},
    App, fx,
    icons::{self, Icon},
    theme::{self, ACCENT, ACCENT_HI, AMBER, CYAN, EDGE, FAINT, GREEN, INDIGO, MUTED, RED, SOFT, TEXT, TILE_HOVER},
    widgets,
};
use crate::{
    app::health,
    domain::{
        auth::AuthMode,
        models,
    },
};

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.set_height(30.0);
        ui.label(widgets::text(tr("Outils", "Tools"), 16.5, TEXT).font(theme::bold(16.5)));
        let ok = app.health.iter().filter(|h| h.ok).count();
        if !app.health.is_empty() {
            let color = if ok == app.health.len() { GREEN } else { AMBER };
            widgets::chip(ui, format!("{ok}/{} {}", app.health.len(), tr("actifs", "active")), color);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::icon_button(ui, Icon::Refresh, tr("Relancer les auto-tests", "Re-run self-tests")).clicked() {
                app.refresh_health();
            }
        });
    });
    ui.add_space(4.0);
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        tools(app, ui);
        live_tests(app, ui);
        super::memory::show(app, ui, heading);
        super::toolbox::show(app, ui);
        engine(app, ui);
        network(app, ui);
        economy(app, ui);
        super::plans::show(app, ui, heading);
        verdict(app, ui);
        ui.add_space(16.0);
    });
}

/// Background test watcher of the workspace: plugin, native or built-in, last verdict.
fn live_tests(app: &App, ui: &mut Ui) {
    if !app.settings.live_tests {
        return;
    }
    heading(ui, tr("Tests en continu", "Live tests"));
    let Some(status) = crate::tools::livetest::current(&app.env.workspace()).map(|r| r.status()) else {
        row(ui, true, tr("En attente", "Waiting"), tr("démarre au premier tour qui touche au code", "starts on the first turn that touches code"));
        return;
    };
    let how = match status.mode {
        crate::tools::livetest::Mode::Native => tr("watcher natif", "native watcher"),
        crate::tools::livetest::Mode::Builtin => tr("watcher Omega", "Omega watcher"),
    };
    let (ok, detail) = match (&status.error, &status.last, status.running) {
        (Some(e), _, _) => (false, e.clone()),
        (None, _, true) => (true, format!("{how} · {}", tr("exécution en cours…", "running…"))),
        (None, Some(last), false) => {
            let ago = last.finished.elapsed().as_secs();
            (
                last.ok,
                if english() {
                    format!("{how} · {} {ago} s ago", if last.ok { "green" } else { "failed" })
                } else {
                    format!("{how} · {} il y a {ago} s", if last.ok { "vert" } else { "échec" })
                },
            )
        }
        (None, None, false) => (true, format!("{how} · {}", tr("pas encore de résultat", "no result yet"))),
    };
    row(ui, ok, status.plugin, &detail);
    if let Some(hint) = status.hint {
        ui.label(widgets::text(format!("{} {hint}", tr("Watcher natif plus rapide :", "Faster native watcher:")), 10.5, FAINT));
    }
    if let Some(last) = status.last.as_ref().filter(|l| !l.ok) {
        ui.label(widgets::text(crate::tools::compress::clip(&last.summary, 600), 10.5, FAINT));
    }
}

/// Self-test groups and names are French in the runtime: their English label, when the interface is in English.
fn health_label(fr: &'static str) -> &'static str {
    if !english() {
        return fr;
    }
    match fr {
        "Moteurs IA" => "AI engines",
        "Économie tokens" => "Token savings",
        "Sécurité" => "Security",
        "Délégation Flash" => "Flash delegation",
        "Graphe d'impact" => "Impact graph",
        "Moteur ripgrep" => "ripgrep engine",
        "Compresseur RTK" => "RTK compressor",
        "Encodeur TOON" => "TOON encoder",
        "Cache préfixe KV" => "KV prefix cache",
        "Style de sortie" => "Output style",
        "Règles projet" => "Project rules",
        "Kits design & web" => "Design & web kits",
        "Tests en continu" => "Live tests",
        "Recherche anonyme" => "Anonymous search",
        "Web local (BM25)" => "Local web (BM25)",
        "Scanner statique" => "Static scanner",
        "Garde shell + sandbox" => "Shell guard + sandbox",
        other => other,
    }
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        widgets::section(ui, text);
    });
    ui.add_space(4.0);
}

/// Status line: breathing dot, name, detail elided to the panel width.
fn row(ui: &mut Ui, ok: bool, name: &str, detail: &str) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
    let k = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    if k > 0.0 {
        p.rect_filled(rect, 9.0, TILE_HOVER.gamma_multiply(k));
    }
    widgets::status(p, rect.left_center() + vec2(12.0, 0.0), if ok { GREEN } else { RED }, fx::now(ui.ctx()));
    let x = rect.left() + 28.0;
    let width = rect.right() - x - 8.0;
    let title = widgets::line(ui, name, FontId::proportional(13.0), if ok { TEXT } else { SOFT }, width);
    let color = if ok { FAINT } else { RED.gamma_multiply(0.85) };
    let sub = widgets::line(ui, detail, FontId::proportional(11.0), color, width);
    let elided = sub.elided;
    let p = ui.painter();
    p.galley(pos2(x, rect.top() + 4.0), title, TEXT);
    p.galley(pos2(x, rect.top() + 21.0), sub, color);
    if elided {
        resp.on_hover_text(detail);
    }
}

fn tools(app: &mut App, ui: &mut Ui) {
    if app.health.is_empty() {
        ui.add_space(10.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
        let t = fx::now(ui.ctx());
        fx::spinner(ui.painter(), rect.left_center() + vec2(12.0, 0.0), 6.0, 2.0, t);
        let job = fx::shimmer("Auto-tests en cours…", FontId::proportional(13.0), MUTED, TEXT, t);
        let galley = ui.painter().layout_job(job);
        ui.painter().galley(rect.left_center() + vec2(28.0, -galley.size().y / 2.0), galley, MUTED);
        return;
    }
    let mut group = "";
    let mut create_rules = false;
    for h in &app.health {
        if h.group != group {
            heading(ui, health_label(h.group));
            group = h.group;
        }
        row(ui, h.ok, health_label(h.name), &h.detail);
        if h.name == health::RULES_CHECK && !h.ok {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                create_rules = widgets::pill(ui, Some(Icon::Plus), tr("Créer OMEGA.md (modèle)", "Create OMEGA.md (template)"), ACCENT_HI, false).clicked();
            });
            ui.add_space(2.0);
        }
    }
    if create_rules {
        app.create_rules_file();
    }
}

/// Provider + exact model id + effort: the always-visible answer to "which model runs this?".
fn engine(app: &App, ui: &mut Ui) {
    heading(ui, tr("Moteur actif", "Active engine"));
    let engine = app.settings.engine;
    let model = app.settings.model(engine).to_owned();
    let effort = app.settings.effort.get(engine);
    let mode = match app.settings.auth_mode(engine) {
        AuthMode::Subscription if engine.subscription_capable() => tr("abonnement", "subscription"),
        _ => tr("clé API", "API key"),
    };
    widgets::card(ui, EDGE, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (badge, _) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::hover());
            let p = ui.painter();
            fx::glow(p, badge, 12.0, ACCENT.gamma_multiply(0.25), 16);
            fx::gradient_rect(p, badge, 12.0, ACCENT, INDIGO);
            p.add(fx::edge_light(badge, 12.0, 80.0, 10.0));
            icons::paint(p, badge.shrink(10.0), Icon::Bolt, Color32::WHITE);
            ui.add_space(2.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                widgets::truncated(ui, &models::name(engine, &model), theme::bold(14.0), TEXT);
                widgets::truncated(ui, &model, FontId::monospace(11.0), SOFT);
            });
        });
        ui.add_space(6.0);
        widgets::truncated(
            ui,
            &format!("{} · effort {} · {mode}", engine.label(), effort.label()),
            FontId::proportional(11.5),
            MUTED,
        );
    });
}

fn network(app: &App, ui: &mut Ui) {
    heading(ui, tr("Réseau & anonymat", "Network & privacy"));
    match &app.vpn {
        Some(v) => {
            let detail = [v.tunnel.as_deref(), v.country.as_deref(), v.org.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            row(ui, v.active, "VPN", if detail.is_empty() { tr("aucun tunnel détecté", "no tunnel detected") } else { &detail });
        }
        None => row(ui, false, "VPN", tr("détection…", "detecting…")),
    }
    let proxy = app.settings.proxy.trim();
    row(ui, !proxy.is_empty(), "Proxy / Tor", if proxy.is_empty() { tr("connexion directe", "direct connection") } else { proxy });
    row(ui, true, tr("Télémétrie", "Telemetry"), tr("aucune · données 100 % locales", "none · data 100 % local"));
    row(
        ui,
        !app.settings.bypass,
        tr("Autorisations", "Permissions"),
        if app.settings.bypass { tr("BYPASS : tout est accepté sans demander", "BYPASS: everything accepted without asking") } else { tr("demandées pour les actions risquées", "asked for risky actions") },
    );
    row(
        ui,
        app.settings.require_vpn,
        "Kill-switch VPN",
        if app.settings.require_vpn { tr("envoi bloqué sans VPN", "sending blocked without VPN") } else { tr("désactivé", "off") },
    );
}

/// Key on the left, value right-aligned.
fn line(ui: &mut Ui, key: &str, value: String) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    let p = ui.painter();
    p.text(rect.left_center(), eframe::egui::Align2::LEFT_CENTER, key, FontId::proportional(12.5), MUTED);
    p.text(rect.right_center(), eframe::egui::Align2::RIGHT_CENTER, value, FontId::monospace(12.0), TEXT);
}

fn economy(app: &App, ui: &mut Ui) {
    let Some(conv) = app.current_conv() else { return };
    let u = conv.usage;
    heading(ui, tr("Économie de la session", "Session savings"));
    widgets::card(ui, EDGE, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 3.0;
        line(ui, tr("Entrée", "Input"), widgets::human(u.prompt));
        line(ui, tr("Sortie", "Output"), widgets::human(u.completion));
        line(ui, tr("Raisonnement", "Reasoning"), widgets::human(u.reasoning));
        line(ui, tr("Jev (contrôle)", "Jev (control)"), widgets::human(u.jev));
        line(ui, tr("Écrit en cache", "Written to cache"), widgets::human(u.written));
        if conv.savings.requests > 0 {
            line(ui, tr("Économie mesurée", "Measured savings"), format!("{:.0} % {}", conv.savings.saved_share() * 100.0, tr("de l'entrée", "of input")));
        }
        line(ui, tr("Outils chargés", "Tools loaded"), conv.toolset.len().to_string());
        line(ui, tr("Contexte actuel", "Current context"), format!("{} / {}", widgets::human(conv.context), widgets::human(app.settings.compact_at)));
        if let Some(s) = &conv.summary {
            line(ui, tr("Résumé actif", "Active summary"), format!("{} {}", s.upto, tr("msg condensés", "msgs condensed")));
        }
        ui.add_space(8.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
        let p = ui.painter();
        p.text(rect.left_center(), eframe::egui::Align2::LEFT_CENTER, "Cache KV", FontId::proportional(12.0), CYAN);
        let ratio = format!("{:.0} % {}", u.cache_ratio() * 100.0, tr("de l'entrée", "of input"));
        p.text(rect.right_center(), eframe::egui::Align2::RIGHT_CENTER, ratio, FontId::monospace(11.5), CYAN);
        ui.add_space(2.0);
        widgets::bar(ui, u.cache_ratio(), CYAN, 6.0);
    });
}

fn verdict(app: &App, ui: &mut Ui) {
    heading(ui, tr("Dernier verdict Jev", "Last Jev verdict"));
    let Some(route) = app.current_conv().and_then(|c| c.messages.iter().rev().find_map(|m| m.route.as_ref())) else {
        widgets::card(ui, EDGE, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                icons::paint(ui.painter(), rect, Icon::Sparkle, FAINT);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(widgets::text(tr("Aucun routage pour l'instant", "No routing yet"), 12.5, SOFT));
                    ui.label(widgets::text(tr("Jev se prononce au premier message envoyé.", "Jev decides on the first message."), 11.0, FAINT));
                });
            });
        });
        return;
    };
    widgets::card(ui, EDGE, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(widgets::text(route.skill.label(), 15.0, TEXT).font(theme::bold(15.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (label, color, icon) = match route.source {
                    crate::domain::route::Source::Jev => ("Jev", CYAN, Icon::Bolt),
                    crate::domain::route::Source::Local => ("local", MUTED, Icon::Gear),
                };
                widgets::chip_icon(ui, Some(icon), label, color);
            });
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(widgets::text(tr("confiance", "confidence"), 11.0, FAINT));
            ui.label(widgets::text(format!("{:.0} %", route.confidence * 100.0), 11.5, TEXT).monospace());
            if route.risky() {
                widgets::chip_icon(ui, Some(Icon::Shield), tr("revue sécurité", "security review"), RED);
            }
        });
        ui.add_space(10.0);
        let tools = app.current_conv().map_or(0, |c| c.toolset.len());
        ui.columns(2, |cols| {
            cols[0].spacing_mut().item_spacing.y = 3.0;
            cols[1].spacing_mut().item_spacing.y = 3.0;
            let left = &mut cols[0];
            stat(left, "Code", route.code, ACCENT_HI);
            left.add_space(6.0);
            stat(left, tr("Risque", "Risk"), route.risk, RED);
            left.add_space(6.0);
            stat(left, "Apps", route.apps, GREEN);
            let right = &mut cols[1];
            stat(right, "Web", route.web, CYAN);
            right.add_space(6.0);
            stat(right, "Impact", route.blast, AMBER);
            right.add_space(6.0);
            stat_count(right, tr("Outils", "Tools"), tools);
        });
        if route.playbook.is_some() || route.needs_web() || route.needs_apps() {
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                if let Some(p) = &route.playbook {
                    widgets::chip_icon(ui, Some(Icon::Book), p.clone(), GREEN);
                }
                if route.needs_web() {
                    widgets::chip_icon(ui, Some(Icon::Globe), "web", CYAN);
                }
                if route.needs_apps() {
                    widgets::chip_icon(ui, Some(Icon::Cube), "apps", GREEN);
                }
            });
        }
    });
}

/// Label, value and a thin gradient bar — readable in a narrow panel.
fn stat(ui: &mut Ui, label: &str, value: f32, color: Color32) {
    let text = if value > 0.0 { format!("{:.0} %", value * 100.0) } else { "—".to_owned() };
    stat_line(ui, label, text, if value > 0.0 { color } else { FAINT });
    widgets::bar(ui, value, color, 4.0);
}

fn stat_count(ui: &mut Ui, label: &str, count: usize) {
    stat_line(ui, label, count.to_string(), TEXT);
    widgets::bar(ui, (count as f32 / 8.0).min(1.0), MUTED, 4.0);
}

fn stat_line(ui: &mut Ui, label: &str, value: String, color: Color32) {
    let (rect, _): (Rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    let p = ui.painter();
    p.text(rect.left_center(), eframe::egui::Align2::LEFT_CENTER, label, FontId::proportional(10.5), FAINT);
    p.text(rect.right_center(), eframe::egui::Align2::RIGHT_CENTER, value, FontId::monospace(11.0), color);
}
