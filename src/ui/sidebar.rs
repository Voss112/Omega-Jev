//! Left rail: new chat, search, history by date, VPN, workspace, settings.

use std::time::{Duration, Instant};

use chrono::{DateTime, Local, NaiveDate, Utc};
use eframe::egui::{
    Align2, FontId, Frame, Id, Mesh, Rect, Response, ScrollArea, Sense, Shape, Stroke, StrokeKind, TextEdit, Ui,
    epaint::RectShape, pos2, vec2,
};
use uuid::Uuid;

use super::{
    App,
    i18n::{english, tr}, fx,
    icons::{self, Icon},
    theme::{self, ACCENT, ACCENT_HI, CYAN, EDGE, EDGE_HI, FAINT, GREEN, INPUT, MUTED, RED, SOFT, TEXT, TILE_ACTIVE, TILE_HOVER},
    widgets,
};
use crate::domain::chat::Conversation;

const FOOTER: f32 = 162.0;
const ROW_H: f32 = 36.0;
const DELETE_CONFIRM: Duration = Duration::from_secs(3);

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.add_space(14.0);
    if widgets::primary_icon(ui, Some(Icon::Plus), tr("Nouvelle discussion", "New chat"), ui.available_width()).on_hover_text("Ctrl+N").clicked()
    {
        app.new_chat();
    }
    ui.add_space(10.0);
    search(app, ui);
    ui.add_space(4.0);
    ScrollArea::vertical()
        .max_height((ui.available_height() - FOOTER).max(80.0))
        .auto_shrink([false, false])
        .show(ui, |ui| history(app, ui));
    footer(app, ui);
}

fn search(app: &mut App, ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
    let id = Id::unique("history-search");
    let focused = ui.memory(|m| m.has_focus(id));
    let k = ui.ctx().animate_bool_with_time(id.with("focus"), focused, 0.15);
    let p = ui.painter();
    if k > 0.0 {
        fx::glow(p, rect, 11.0, ACCENT.gamma_multiply(0.22 * k), 14);
    }
    p.rect(rect, 11.0, INPUT, Stroke::new(1.0, fx::mix(EDGE, ACCENT.gamma_multiply(0.75), k)), StrokeKind::Inside);
    icons::at(p, rect.left_center() + vec2(18.0, 0.0), 15.0, Icon::Search, fx::mix(FAINT, ACCENT_HI, k));
    let clear = !app.search.is_empty();
    let edit = Rect::from_min_max(rect.min + vec2(34.0, 0.0), rect.max - vec2(if clear { 34.0 } else { 10.0 }, 0.0));
    ui.put(
        edit,
        TextEdit::singleline(&mut app.search)
            .id(id)
            .frame(Frame::NONE)
            .hint_text(tr("Rechercher", "Search"))
            .align(Align2::LEFT_CENTER)
            .font(FontId::proportional(13.5))
            .desired_width(edit.width()),
    );
    if clear {
        let button = Rect::from_center_size(rect.right_center() - vec2(18.0, 0.0), vec2(24.0, 24.0));
        let mut child = ui.new_child(eframe::egui::UiBuilder::new().max_rect(button));
        if widgets::icon_button_sized(&mut child, Icon::Close, 24.0, FAINT, false).on_hover_text(tr("Effacer", "Clear")).clicked() {
            app.search.clear();
        }
    }
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
    // The selection pill is painted under the rows once their positions are known, then slides between them.
    let pill = ui.painter().add(Shape::Noop);
    let origin = ui.min_rect().top();
    let mut selected_y = None;
    let t = fx::now(ui.ctx());
    for conv in app.convs.iter().filter(|c| needle.is_empty() || c.title.to_lowercase().contains(&needle)) {
        let label = bucket(today, conv.updated);
        if label != bucket_shown {
            ui.add_space(if bucket_shown.is_empty() { 8.0 } else { 14.0 });
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                widgets::section(ui, label);
            });
            ui.add_space(2.0);
            bucket_shown = label;
        }
        let selected = app.current == Some(conv.id);
        let running = app.live.contains_key(&conv.id);
        let (clicked, rect) = row(ui, conv, selected, running, armed == Some(conv.id), t);
        if selected {
            selected_y = Some(rect.top() - origin);
        }
        action = action.or(clicked);
    }
    if let Some(y) = selected_y {
        let y = ui.ctx().animate_value_with_time(Id::unique("history-pill"), y, 0.22);
        let rect = Rect::from_min_size(pos2(ui.min_rect().left(), origin + y), vec2(ui.min_rect().width(), ROW_H));
        ui.painter().set(pill, selection(rect));
    }
    if app.convs.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
            icons::paint(ui.painter(), rect, Icon::Bubble, FAINT);
            ui.add_space(6.0);
            ui.label(widgets::text(tr("Aucune discussion pour l'instant.", "No chats yet."), 12.5, FAINT));
        });
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

/// Glass pill with a violet→cyan accent bar.
fn selection(rect: Rect) -> Shape {
    let bar = Rect::from_min_size(rect.left_top() + vec2(0.0, 9.0), vec2(3.0, rect.height() - 18.0));
    let mut mesh = Mesh::default();
    let base = 0;
    for (pos, color) in
        [(bar.left_top(), ACCENT_HI), (bar.right_top(), ACCENT_HI), (bar.right_bottom(), CYAN), (bar.left_bottom(), CYAN)]
    {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(base, base + 1, base + 2);
    mesh.add_triangle(base, base + 2, base + 3);
    Shape::Vec(vec![
        RectShape::new(rect, 10.0, TILE_ACTIVE, Stroke::new(1.0, EDGE_HI), StrokeKind::Inside).into(),
        Shape::Rect(
            eframe::egui::Shadow { offset: [0, 0], blur: 10, spread: 0, color: fx::additive(ACCENT.gamma_multiply(0.5)) }
                .as_shape(bar, 2.0),
        ),
        Shape::mesh(mesh),
    ])
}

/// Returns ((id, delete?) when clicked, row rect). `armed`: a delete click is awaiting confirmation.
fn row(ui: &mut Ui, conv: &Conversation, selected: bool, running: bool, armed: bool, t: f64) -> (Option<(Uuid, bool)>, Rect) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    ui.add_space(2.0);
    let del_rect = Rect::from_center_size(rect.right_center() - vec2(18.0, 0.0), vec2(26.0, 26.0));
    let del = ui.interact(del_rect, ui.scope_id().with(("del", conv.id)), Sense::click());
    let hovered = resp.hovered() || del.hovered() || armed;
    let k = ui.ctx().animate_bool_with_time(resp.id, hovered, 0.12);
    let p = ui.painter();
    if !selected && k > 0.0 {
        p.rect_filled(rect, 10.0, TILE_HOVER.gamma_multiply(k));
    }
    let color = if armed {
        RED
    } else if selected {
        TEXT
    } else {
        fx::mix(SOFT, TEXT, k * 0.7)
    };
    let title = if armed { tr("Re-cliquer pour supprimer", "Click again to delete") } else { conv.title.as_str() };
    let reserve = if hovered || running { 40.0 } else { 14.0 };
    let galley = widgets::line(ui, title, FontId::proportional(13.5), color, rect.width() - 14.0 - reserve);
    p.galley(rect.left_center() + vec2(14.0, -galley.size().y / 2.0), galley, color);
    if k > 0.02 {
        let danger = del.hovered() || armed;
        if danger {
            p.rect_filled(del_rect, 8.0, RED.gamma_multiply(0.14 * k));
        }
        let color = if danger { RED } else { FAINT };
        icons::at(p, del_rect.center(), 15.0, Icon::Trash, color.gamma_multiply(k));
    } else if running {
        widgets::status(p, del_rect.center(), CYAN, t);
    }
    let del = del.on_hover_text(tr("Supprimer définitivement (confirmer par un 2e clic)", "Delete permanently (confirm with a 2nd click)"));
    let clicked = if del.clicked() { Some((conv.id, true)) } else { resp.clicked().then_some((conv.id, false)) };
    (clicked, rect)
}

fn bucket(today: NaiveDate, at: DateTime<Utc>) -> &'static str {
    match (today - at.with_timezone(&Local).date_naive()).num_days() {
        ..=0 => tr("Aujourd'hui", "Today"),
        1 => tr("Hier", "Yesterday"),
        2..=7 => tr("7 derniers jours", "Last 7 days"),
        8..=30 => tr("30 derniers jours", "Last 30 days"),
        _ => tr("Plus ancien", "Older"),
    }
}

fn footer(app: &mut App, ui: &mut Ui) {
    ui.add_space(8.0);
    let (ok, label, place, detail) = match &app.vpn {
        Some(v) => {
            let place = [v.country.as_deref(), v.org.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            let ip = v.ip.as_deref().unwrap_or(tr("non vérifiée", "unchecked"));
            let tunnel = v.tunnel.as_deref().unwrap_or(tr("aucun", "none"));
            let org = v.org.as_deref().unwrap_or("?");
            let detail = if english() {
                format!("IP: {ip}\nTunnel: {tunnel}\nProvider: {org}")
            } else {
                format!("IP : {ip}\nTunnel : {tunnel}\nOpérateur : {org}")
            };
            (v.active, if v.active { tr("VPN actif", "VPN on") } else { tr("Sans VPN", "No VPN") }, place, detail)
        }
        None => (false, "VPN", tr("détection…", "detecting…").to_owned(), String::new()),
    };
    vpn_card(ui, ok, label, &place).on_hover_text(detail);
    ui.add_space(8.0);
    let workspace = app.env.workspace();
    let name = workspace.file_name().map_or_else(|| workspace.display().to_string(), |n| n.to_string_lossy().into_owned());
    if nav(ui, Icon::Folder, &name)
        .on_hover_text(if english() {
            format!("Workspace: {}\nClick to change", workspace.display())
        } else {
            format!("Dossier de travail : {}\nCliquer pour changer", workspace.display())
        })
        .clicked()
    {
        app.pick_workspace();
    }
    ui.add_space(2.0);
    if nav(ui, Icon::Gear, tr("Paramètres", "Settings")).clicked() {
        app.settings_draft = Some(app.settings.clone());
    }
    ui.add_space(12.0);
}

fn vpn_card(ui: &mut Ui, ok: bool, label: &str, place: &str) -> Response {
    let color = if ok { GREEN } else { RED };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::hover());
    let p = ui.painter();
    fx::gradient_rect(p, rect, 14.0, color.gamma_multiply(0.13), color.gamma_multiply(0.03));
    p.rect_stroke(rect, 14.0, Stroke::new(1.0, color.gamma_multiply(0.30)), StrokeKind::Inside);
    let badge = Rect::from_center_size(rect.left_center() + vec2(26.0, 0.0), vec2(32.0, 32.0));
    p.rect_filled(badge, 10.0, color.gamma_multiply(0.16));
    icons::paint(p, badge.shrink(8.0), Icon::Shield, color);
    let x = badge.right() + 10.0;
    let width = rect.right() - x - 30.0;
    let title = widgets::line(ui, label, theme::bold(12.5), color, width);
    let sub = widgets::line(ui, place, FontId::proportional(11.0), MUTED, width);
    let p = ui.painter();
    p.galley(pos2(x, rect.center().y - title.size().y + 1.0), title, color);
    p.galley(pos2(x, rect.center().y + 2.0), sub, MUTED);
    widgets::status(p, rect.right_center() - vec2(18.0, 0.0), color, fx::now(ui.ctx()));
    resp
}

/// Footer navigation row: icon + label, glass on hover.
fn nav(ui: &mut Ui, icon: Icon, label: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let k = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    if k > 0.0 {
        p.rect(rect, 10.0, TILE_HOVER.gamma_multiply(k), Stroke::new(1.0, EDGE.gamma_multiply(k)), StrokeKind::Inside);
    }
    let fg = fx::mix(SOFT, TEXT, k);
    icons::at(p, rect.left_center() + vec2(18.0, 0.0), 16.0, icon, fx::mix(MUTED, ACCENT_HI, k));
    let galley = widgets::line(ui, label, FontId::proportional(13.5), fg, rect.width() - 60.0);
    ui.painter().galley(rect.left_center() + vec2(38.0, -galley.size().y / 2.0), galley, fg);
    icons::chevron(ui.painter(), rect.right_center() - vec2(16.0, 0.0), 12.0, 0.0, FAINT.gamma_multiply(k));
    resp
}
