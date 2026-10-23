//! Left rail: brand, new chat, search, history by date, VPN, workspace, settings.

use std::time::{Duration, Instant};

use chrono::{DateTime, Local, NaiveDate, Utc};
use eframe::egui::{Align2, Button, CornerRadius, FontId, Rect, RichText, ScrollArea, Sense, TextEdit, Ui, vec2};
use uuid::Uuid;

use super::{
    App,
    theme::{self, ACCENT, CARD, CYAN, FAINT, GREEN, MUTED, RAISED, RED, SOFT, TEXT},
    widgets,
};
use crate::domain::chat::Conversation;

const FOOTER: f32 = 184.0;
const DELETE_CONFIRM: Duration = Duration::from_secs(3);

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.add_space(18.0);
    ui.horizontal(|ui| {
        widgets::logo(ui, 36.0);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.label(RichText::new("Omega").font(theme::bold(19.0)).color(TEXT));
            ui.label(RichText::new(format!("Jev × {} · local", app.settings.engine.label())).size(11.5).color(MUTED));
        });
    });
    ui.add_space(16.0);
    if widgets::primary(ui, "+  Nouvelle discussion", ui.available_width()).clicked() {
        app.new_chat();
    }
    ui.add_space(10.0);
    ui.add(TextEdit::singleline(&mut app.search).hint_text("🔍  Rechercher").desired_width(f32::INFINITY));
    ui.add_space(6.0);
    ScrollArea::vertical()
        .max_height((ui.available_height() - FOOTER).max(80.0))
        .auto_shrink([false, false])
        .show(ui, |ui| history(app, ui));
    footer(app, ui);
}

fn history(app: &mut App, ui: &mut Ui) {
    let needle = app.search.to_lowercase();
    let today = Local::now().date_naive();
    let mut bucket_shown = "";
    let mut action: Option<(Uuid, bool)> = None;
    let armed = app.delete_armed.filter(|(_, at)| at.elapsed() < DELETE_CONFIRM).map(|(id, _)| id);
    if armed.is_some() {
        ui.ctx().request_repaint_after(DELETE_CONFIRM);
    }
    for conv in app.convs.iter().filter(|c| needle.is_empty() || c.title.to_lowercase().contains(&needle)) {
        let label = bucket(today, conv.updated);
        if label != bucket_shown {
            ui.add_space(10.0);
            widgets::section(ui, label);
            ui.add_space(2.0);
            bucket_shown = label;
        }
        let running = app.live.contains_key(&conv.id);
        if let Some(a) = row(ui, conv, app.current == Some(conv.id), running, armed == Some(conv.id)) {
            action = Some(a);
        }
    }
    if app.convs.is_empty() {
        ui.add_space(16.0);
        ui.label(RichText::new("Aucune discussion pour l'instant.").size(12.5).color(FAINT));
    }
    match action {
        // Deletion is irreversible: the first click arms, a second click within DELETE_CONFIRM deletes.
        Some((id, true)) if armed == Some(id) => {
            app.delete_armed = None;
            app.delete_chat(id);
        }
        Some((id, true)) => app.delete_armed = Some((id, Instant::now())),
        Some((id, false)) => app.select(id),
        None => {}
    }
}

/// Returns (id, delete?) when clicked. `armed`: a delete click is awaiting confirmation.
fn row(ui: &mut Ui, conv: &Conversation, selected: bool, running: bool, armed: bool) -> Option<(Uuid, bool)> {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    let del_rect = Rect::from_center_size(rect.right_center() - vec2(16.0, 0.0), vec2(24.0, 24.0));
    let del = ui.interact(del_rect, ui.scope_id().with(("del", conv.id)), Sense::click());
    let hovered = resp.hovered() || del.hovered() || armed;
    let painter = ui.painter();
    let fill = if selected {
        RAISED
    } else if hovered {
        CARD
    } else {
        eframe::egui::Color32::TRANSPARENT
    };
    painter.rect_filled(rect, CornerRadius::same(8), fill);
    if selected {
        let bar = Rect::from_min_size(rect.left_top() + vec2(0.0, 9.0), vec2(3.0, rect.height() - 18.0));
        painter.rect_filled(bar, CornerRadius::same(2), ACCENT);
    }
    let max_chars = if hovered || running { 27 } else { 31 };
    let title: String = if armed {
        "Re-cliquer 🗑 pour supprimer".to_owned()
    } else if conv.title.chars().count() > max_chars {
        conv.title.chars().take(max_chars - 1).chain(std::iter::once('…')).collect()
    } else {
        conv.title.clone()
    };
    painter.text(
        rect.left_center() + vec2(12.0, 0.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(13.5),
        if armed {
            RED
        } else if selected {
            TEXT
        } else {
            SOFT
        },
    );
    if hovered {
        painter.text(
            del_rect.center(),
            Align2::CENTER_CENTER,
            "🗑",
            FontId::proportional(13.0),
            if del.hovered() || armed { RED } else { FAINT },
        );
    } else if running {
        painter.circle_filled(del_rect.center(), 3.5, CYAN);
    }
    let del = del.on_hover_text("Supprimer définitivement (confirmer par un 2e clic)");
    if del.clicked() { Some((conv.id, true)) } else { resp.clicked().then_some((conv.id, false)) }
}

fn bucket(today: NaiveDate, at: DateTime<Utc>) -> &'static str {
    match (today - at.with_timezone(&Local).date_naive()).num_days() {
        ..=0 => "Aujourd'hui",
        1 => "Hier",
        2..=7 => "7 derniers jours",
        8..=30 => "30 derniers jours",
        _ => "Plus ancien",
    }
}

fn footer(app: &mut App, ui: &mut Ui) {
    ui.add_space(8.0);
    let (ok, label, detail) = match &app.vpn {
        Some(v) => {
            let place = [v.country.as_deref(), v.org.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            let detail = format!(
                "IP : {}\nTunnel : {}\nOpérateur : {}",
                v.ip.as_deref().unwrap_or("non vérifiée"),
                v.tunnel.as_deref().unwrap_or("aucun"),
                v.org.as_deref().unwrap_or("?")
            );
            (v.active, if v.active { format!("VPN actif  {place}") } else { format!("Sans VPN  {place}") }, detail)
        }
        None => (false, "VPN : détection…".to_owned(), String::new()),
    };
    widgets::card(ui, if ok { GREEN.gamma_multiply(0.4) } else { RED.gamma_multiply(0.4) }, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            widgets::dot(ui, ok);
            ui.label(RichText::new(label).size(12.5).color(if ok { GREEN } else { RED }));
        });
    })
    .response
    .on_hover_text(detail);
    ui.add_space(6.0);
    let workspace = app.env.workspace();
    let name = workspace.file_name().map_or_else(|| workspace.display().to_string(), |n| n.to_string_lossy().into_owned());
    let button =
        Button::new(RichText::new(format!("📁  {name}")).size(13.0).color(SOFT)).min_size(vec2(ui.available_width(), 30.0));
    if ui.add(button).on_hover_text(format!("Dossier de travail : {}\nCliquer pour changer", workspace.display())).clicked() {
        app.pick_workspace();
    }
    let button = Button::new(RichText::new("⚙  Paramètres").size(13.0).color(SOFT)).min_size(vec2(ui.available_width(), 30.0));
    if ui.add(button).clicked() {
        app.settings_draft = Some(app.settings.clone());
    }
    ui.add_space(12.0);
}
