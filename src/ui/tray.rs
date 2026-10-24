//! Notification-area icon and Windows start-up, kept in line with the settings.

use std::sync::{
    Arc,
    atomic::{AtomicIsize, Ordering},
    mpsc::{self, Receiver},
};

use eframe::egui::{Context, ViewportCommand};

use super::App;
use crate::infra::{
    autostart,
    tray::{self, TrayEvent, TrayIcon},
};

pub struct Tray {
    _icon: TrayIcon,
    events: Receiver<TrayEvent>,
    /// "Quitter" chosen: the next close request really exits.
    quitting: bool,
    /// Native handle of the main window, shared with the tray thread.
    main: Arc<AtomicIsize>,
}

/// Launched by the Run key at logon: start hidden in the tray.
pub fn background_launch() -> bool {
    std::env::args().any(|a| a == "--background")
}

impl App {
    /// Creates or removes the icon and the Run value to match the settings.
    pub(super) fn apply_system(&mut self, ctx: &Context) {
        self.sync_tray(ctx);
        if let Err(e) = autostart::apply(self.settings.autostart) {
            self.notify(format!("Démarrage avec Windows : {e:#}"));
        }
    }

    /// Icon created or removed when the setting changes.
    pub(super) fn sync_tray(&mut self, ctx: &Context) {
        if self.settings.tray != self.tray.is_some() {
            self.tray = self.settings.tray.then(|| spawn(ctx)).flatten();
        }
    }

    /// Tray clicks, and the window close button hiding to the tray instead of quitting.
    pub(super) fn pump_tray(&mut self, ctx: &Context, frame: &eframe::Frame) {
        let Some(tray) = &mut self.tray else { return };
        if let Some(hwnd) = native_handle(frame) {
            tray.main.store(hwnd, Ordering::Release);
        }
        while let Ok(event) = tray.events.try_recv() {
            match event {
                TrayEvent::Open => {
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                TrayEvent::Quit => {
                    tray.quitting = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !tray.quitting {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }
    }
}

/// Win32 handle of the eframe window, so the tray thread can show it while egui is asleep.
fn native_handle(frame: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

fn spawn(ctx: &Context) -> Option<Tray> {
    let (tx, events) = mpsc::channel();
    let wake = ctx.clone();
    let main = Arc::new(AtomicIsize::new(0));
    match tray::spawn(tx, Box::new(move || wake.request_repaint()), main.clone()) {
        Ok(icon) => Some(Tray { _icon: icon, events, quitting: false, main }),
        Err(e) => {
            eprintln!("{e:#}");
            None
        }
    }
}
