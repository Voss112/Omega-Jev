//! Frameless window chrome: integrated title bar, window controls, edge resizing.

use eframe::egui::{
    Align2, Area, Color32, Context, CursorIcon, FontId, Id, Order, PointerButton, Rect, ResizeDirection, Sense, Ui,
    ViewportCommand, pos2, vec2,
};

use super::{
    App, fx,
    icons::{self, Icon},
    theme::{self, FAINT, MUTED, TEXT},
    widgets,
};
use crate::domain::models;

pub const TITLE_H: f32 = 40.0;
const BUTTON_W: f32 = 46.0;
const EDGE: f32 = 5.0;
const CORNER: f32 = 12.0;

pub fn title_bar(app: &App, ui: &mut Ui) {
    let rect = ui.max_rect();
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let drag = Rect::from_min_max(rect.min, pos2(rect.right() - BUTTON_W * 3.0, rect.bottom()));
    let resp = ui.interact(drag, Id::unique("title-drag"), Sense::click_and_drag());
    if resp.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if resp.drag_started_by(PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }

    let p = ui.painter();
    let t = fx::now(&ctx);
    let logo = Rect::from_center_size(pos2(rect.left() + 28.0, rect.center().y), vec2(22.0, 22.0));
    widgets::paint_logo(p, logo, t);
    let name = p.layout_no_wrap("Omega".into(), theme::bold(14.0), TEXT);
    let x = logo.right() + 10.0;
    let name_w = name.size().x;
    p.galley(pos2(x, rect.center().y - name.size().y / 2.0), name, TEXT);
    let engine = app.settings.engine;
    let subtitle = format!("Jev  ×  {}", models::name(engine, app.settings.model(engine)));
    p.text(pos2(x + name_w + 12.0, rect.center().y), Align2::LEFT_CENTER, subtitle, FontId::proportional(12.0), FAINT);

    let mut x = rect.right() - BUTTON_W * 3.0;
    for (icon, cmd) in [
        (Icon::Minimize, ViewportCommand::Minimized(true)),
        (if maximized { Icon::Restore } else { Icon::Maximize }, ViewportCommand::Maximized(!maximized)),
        (Icon::Close, ViewportCommand::Close),
    ] {
        let button = Rect::from_min_size(pos2(x, rect.top()), vec2(BUTTON_W, TITLE_H));
        if control(ui, button, icon) {
            ctx.send_viewport_cmd(cmd);
        }
        x += BUTTON_W;
    }
}

fn control(ui: &mut Ui, rect: Rect, icon: Icon) -> bool {
    let resp = ui.interact(rect, Id::unique(("window-control", icon as u8)), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    let p = ui.painter();
    let close = icon == Icon::Close;
    if hover > 0.0 {
        let fill = if close { Color32::from_rgb(0xD9, 0x34, 0x3B) } else { Color32::from_white_alpha(18) };
        p.rect_filled(rect, 0.0, fill.gamma_multiply(hover));
    }
    let color = fx::mix(MUTED, if close { Color32::WHITE } else { TEXT }, hover);
    icons::at(p, rect.center(), 15.0, icon, color);
    resp.clicked()
}

/// Invisible grips along the window border; they hand the resize over to the OS.
pub fn resize_edges(ctx: &Context) {
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false));
    if maximized {
        return;
    }
    let r = ctx.content_rect();
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    let zones = [
        (ResizeDirection::NorthWest, Rect::from_min_max(pos2(l, t), pos2(l + CORNER, t + CORNER)), CursorIcon::ResizeNorthWest),
        (ResizeDirection::NorthEast, Rect::from_min_max(pos2(rt - CORNER, t), pos2(rt, t + CORNER)), CursorIcon::ResizeNorthEast),
        (ResizeDirection::SouthWest, Rect::from_min_max(pos2(l, b - CORNER), pos2(l + CORNER, b)), CursorIcon::ResizeSouthWest),
        (ResizeDirection::SouthEast, Rect::from_min_max(pos2(rt - CORNER, b - CORNER), pos2(rt, b)), CursorIcon::ResizeSouthEast),
        (ResizeDirection::North, Rect::from_min_max(pos2(l + CORNER, t), pos2(rt - CORNER, t + EDGE)), CursorIcon::ResizeNorth),
        (ResizeDirection::South, Rect::from_min_max(pos2(l + CORNER, b - EDGE), pos2(rt - CORNER, b)), CursorIcon::ResizeSouth),
        (ResizeDirection::West, Rect::from_min_max(pos2(l, t + CORNER), pos2(l + EDGE, b - CORNER)), CursorIcon::ResizeWest),
        (ResizeDirection::East, Rect::from_min_max(pos2(rt - EDGE, t + CORNER), pos2(rt, b - CORNER)), CursorIcon::ResizeEast),
    ];
    for (i, (direction, zone, cursor)) in zones.into_iter().enumerate() {
        Area::new(Id::unique(("resize-grip", i)))
            .order(Order::Foreground)
            .constrain(false)
            .default_size(zone.size())
            .fixed_pos(zone.min)
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(Rect::from_min_size(zone.min, zone.size()), Sense::drag());
                if resp.hovered() {
                    ctx.set_cursor_icon(cursor);
                    if ctx.input(|i| i.pointer.primary_pressed()) {
                        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
                    }
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::{RawInput, Rect, pos2, vec2};

    use super::*;

    fn frame(ctx: &Context) {
        let input =
            RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))), ..Default::default() };
        ctx.run_ui(input, |ui| resize_edges(ui.ctx())).textures_delta.clear();
    }

    #[test]
    fn grips_stay_on_the_border() {
        let ctx = Context::default();
        for _ in 0..3 {
            frame(&ctx);
        }
        for p in [pos2(600.0, 700.0), pos2(600.0, 790.0), pos2(20.0, 700.0), pos2(1180.0, 700.0)] {
            assert!(ctx.layer_id_at(p).is_none_or(|l| l.order != Order::Foreground), "grip covers {p:?}");
        }
        assert!(ctx.layer_id_at(pos2(600.0, 798.0)).is_some_and(|l| l.order == Order::Foreground));
    }
}
