//! Resolution adaptation: one zoom per monitor so the layout fits a 1366×768 laptop and stays legible on 4K.

use eframe::egui::{Context, Vec2, ViewportCommand};

/// Smallest logical area the full layout (sidebar, files, chat, tools) needs at zoom 1.
const MIN_FIT: Vec2 = Vec2::new(1280.0, 760.0);
/// Logical area above which the UI grows (4K at 100 % OS scaling ≈ 3840×2160).
const GROW_FROM: Vec2 = Vec2::new(2400.0, 1350.0);
const ZOOM_MIN: f32 = 0.75;
const ZOOM_MAX: f32 = 2.0;
/// Below this width (points) the side columns collapse so the chat keeps room.
pub const NARROW_W: f32 = 1100.0;

/// Zoom for a monitor of `logical` size (points at OS scaling, zoom excluded).
pub fn zoom_for(logical: Vec2) -> f32 {
    let fit = (logical.x / MIN_FIT.x).min(logical.y / MIN_FIT.y);
    let grow = (logical.x / GROW_FROM.x).min(logical.y / GROW_FROM.y);
    let zoom = if fit < 1.0 { fit } else { grow.max(1.0) };
    ((zoom * 20.0).round() / 20.0).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// Remembers the monitor last adapted to; re-zooms when the window moves to another screen.
#[derive(Default)]
pub struct Scaler {
    monitor: Option<Vec2>,
}

impl Scaler {
    pub fn update(&mut self, ctx: &Context) {
        let (monitor, outer) = ctx.input(|i| (i.viewport().monitor_size, i.viewport().outer_rect));
        let Some(monitor) = monitor else { return };
        let logical = monitor * ctx.zoom_factor();
        if self.monitor.is_some_and(|m| (m - logical).length() < 1.0) {
            return;
        }
        let first = self.monitor.is_none();
        self.monitor = Some(logical);
        let zoom = zoom_for(logical);
        ctx.set_zoom_factor(zoom);
        // Default size larger than a small screen: open maximized instead of off-screen.
        let too_big =
            outer.is_some_and(|r| r.width() * ctx.zoom_factor() > logical.x || r.height() * ctx.zoom_factor() > logical.y);
        if first && too_big {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_hd_keeps_native_size() {
        assert_eq!(zoom_for(Vec2::new(1920.0, 1080.0)), 1.0);
        assert_eq!(zoom_for(Vec2::new(2560.0, 1440.0)), 1.05);
    }

    #[test]
    fn small_laptop_shrinks_to_fit() {
        assert_eq!(zoom_for(Vec2::new(1366.0, 768.0)), 1.0);
        assert_eq!(zoom_for(Vec2::new(1280.0, 720.0)), 0.95);
        assert_eq!(zoom_for(Vec2::new(800.0, 600.0)), ZOOM_MIN);
    }

    #[test]
    fn uhd_at_100_percent_grows() {
        assert_eq!(zoom_for(Vec2::new(3840.0, 2160.0)), 1.6);
        assert_eq!(zoom_for(Vec2::new(7680.0, 4320.0)), ZOOM_MAX);
    }
}
