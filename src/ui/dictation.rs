//! Voice dictation in the composer: mic button / Ctrl+M, live level meter, phrases appended to the draft as they are
//! recognised. Enter while dictating stops and sends once the last phrase is written.

use std::{sync::Arc, time::Duration};

use eframe::egui::{self, FontId, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::{
    App,
    i18n::tr, fx,
    icons::{self, Icon},
    theme::{RED, SOFT, TEXT, TILE_ACTIVE},
    widgets,
};
use crate::{
    app::dictation::{self, Dictation, Update},
    infra::voice::model,
};

/// Meter refresh while the microphone is open.
const METER: Duration = Duration::from_millis(50);

pub(super) struct Voice {
    session: Dictation,
    phase: Phase,
    /// Enter was pressed: send the draft as soon as the last phrase is in.
    send_after: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Download(u64),
    Starting,
    Listening,
}

impl Voice {
    pub(super) fn stop(&self) {
        self.session.stop();
    }
}

impl App {
    pub(super) fn dictating(&self) -> bool {
        self.voice.is_some()
    }

    /// Mic button / Ctrl+M: starts, or stops (the end of the speech is still transcribed).
    pub(super) fn toggle_dictation(&mut self, ctx: &egui::Context) {
        if let Some(v) = &self.voice {
            v.stop();
            return;
        }
        // First use downloads the model: the VPN kill-switch applies to that request too.
        if !model::installed(&model::TURBO) && self.settings.require_vpn && !self.vpn.as_ref().is_some_and(|v| v.active) {
            self.notify(tr("Kill-switch VPN : aucun VPN détecté, téléchargement du modèle vocal bloqué.", "VPN kill-switch: no VPN detected, voice model download blocked."));
            return;
        }
        let Some(rt) = self.rt.as_ref().map(|rt| rt.handle().clone()) else { return };
        let wake_ctx = ctx.clone();
        let session = Dictation::start(
            rt,
            self.env.http.clone(),
            self.settings.dictation_language.clone(),
            self.draft.clone(),
            Arc::new(move || wake_ctx.request_repaint()),
        );
        self.voice = Some(Voice { session, phase: Phase::Starting, send_after: false });
        self.focus_composer = true;
    }

    /// Enter while dictating: stop, then send when the transcription is complete.
    pub(super) fn send_after_dictation(&mut self) {
        if let Some(v) = &mut self.voice {
            v.send_after = true;
            v.session.stop();
        }
    }

    /// Applies what the session produced since the last frame.
    pub(super) fn pump_dictation(&mut self, ctx: &egui::Context) {
        let Some(v) = &mut self.voice else { return };
        let updates: Vec<Update> = v.session.updates.try_iter().collect();
        let mut finished = None;
        for update in updates {
            match update {
                Update::Download { done, total } => v.phase = Phase::Download(done * 100 / total.max(1)),
                Update::Listening => v.phase = Phase::Listening,
                Update::Text(text) => dictation::append(&mut self.draft, &text),
                Update::Failed(e) => self.notice = Some((format!("{} : {e}", tr("Dictée", "Dictation")), std::time::Instant::now())),
                Update::Finished => finished = Some(v.send_after),
            }
        }
        match finished {
            Some(send) => {
                self.voice = None;
                if send {
                    self.send();
                }
            }
            None => ctx.request_repaint_after(METER),
        }
    }
}

/// Composer toolbar control: a mic button when idle, a live pill while dictating. `true` when clicked.
pub(super) fn button(app: &App, ui: &mut Ui, t: f64) -> bool {
    let Some(v) = &app.voice else {
        return widgets::icon_button(ui, Icon::Mic, tr("Dicter (Ctrl+M) : parlez, le texte s'écrit au fil des phrases · Entrée envoie", "Dictate (Ctrl+M): speak, the text is written sentence by sentence · Enter sends"))
            .clicked();
    };
    let label = match (v.phase, v.session.stopping()) {
        (Phase::Download(pct), _) => format!("{} {pct} %", tr("Modèle vocal", "Voice model")),
        (_, true) => tr("Transcription…", "Transcribing…").to_owned(),
        (Phase::Starting, false) => tr("Préparation…", "Preparing…").to_owned(),
        (Phase::Listening, false) => tr("Écoute", "Listening").to_owned(),
    };
    let listening = v.phase == Phase::Listening && !v.session.stopping();
    live_pill(ui, &label, if listening { v.session.level() } else { 0.0 }, listening, t)
        .on_hover_text(tr("Cliquer ou Ctrl+M pour arrêter · Entrée arrête et envoie", "Click or Ctrl+M to stop · Enter stops and sends"))
        .clicked()
}

fn live_pill(ui: &mut Ui, label: &str, level: f32, listening: bool, t: f64) -> Response {
    let font = FontId::proportional(13.0);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, TEXT);
    let bars = if listening { 5.0 * 5.0 } else { 14.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(16.0 + 8.0 + galley.size().x + bars + 28.0, 30.0), Sense::click());
    let p = ui.painter();
    let pulse = if listening { 0.5 + 0.5 * (t * 4.0).sin() as f32 } else { 0.0 };
    p.rect(rect, 9.0, TILE_ACTIVE, Stroke::new(1.0, RED.gamma_multiply(0.45 + 0.4 * pulse)), StrokeKind::Inside);
    let mut x = rect.left() + 11.0;
    icons::at(p, pos2(x + 8.0, rect.center().y), 16.0, Icon::Mic, if listening { RED } else { SOFT });
    x += 24.0;
    p.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley.clone(), TEXT);
    x += galley.size().x + 6.0;
    if listening {
        for i in 0..5 {
            // Each bar lights up at a higher loudness, with a slight wobble so silence still looks alive.
            let lit = ((level * 5.0) - i as f32).clamp(0.0, 1.0);
            let h = 4.0 + 12.0 * lit + 1.5 * ((t * 9.0 + f64::from(i)).sin() as f32 + 1.0) * (1.0 - lit) * 0.5;
            let bar = egui::Rect::from_center_size(pos2(x + 2.0 + i as f32 * 5.0, rect.center().y), vec2(3.0, h));
            p.rect_filled(bar, 1.5, fx::mix(SOFT, RED, lit));
        }
    } else {
        fx::spinner(p, pos2(x + 7.0, rect.center().y), 5.0, 1.8, t);
    }
    resp
}
