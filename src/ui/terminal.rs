//! User terminal under the composer: Ctrl+` (or its bar) folds it; commands run in the workspace shell.

use std::collections::VecDeque;

use eframe::egui::{FontId, Frame, Key, Margin, Modifiers, ScrollArea, TextEdit, Ui};

use super::{
    App,
    i18n::tr,
    icons::Icon,
    theme::{self, CYAN, FAINT, MUTED, SOFT, TEXT},
    widgets,
};
use crate::app::Event;

/// Characters of scrollback kept.
const SCROLLBACK: usize = 200_000;
/// Command history entries kept.
const HISTORY: usize = 100;
/// A command may run this long before it is killed.
const TIMEOUT_SECS: usize = 600;

#[derive(Default)]
pub struct Terminal {
    pub open: bool,
    input: String,
    output: String,
    running: bool,
    history: VecDeque<String>,
    /// Position while browsing the history with ↑/↓.
    cursor: Option<usize>,
}

impl Terminal {
    /// Output of a finished command.
    pub fn finished(&mut self, text: &str) {
        self.running = false;
        self.output.push_str(text.trim_end());
        self.output.push_str("\n\n");
        let extra = self.output.len().saturating_sub(SCROLLBACK);
        if extra > 0 {
            let cut = self.output.ceil_char_boundary(extra);
            self.output.drain(..cut);
        }
    }

    fn remember(&mut self, cmd: &str) {
        if self.history.back().is_none_or(|last| last != cmd) {
            self.history.push_back(cmd.to_owned());
        }
        if self.history.len() > HISTORY {
            self.history.pop_front();
        }
        self.cursor = None;
    }

    fn browse(&mut self, up: bool) {
        let len = self.history.len();
        if len == 0 {
            return;
        }
        let next = match (self.cursor, up) {
            (None, true) => Some(len - 1),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < len => Some(i + 1),
            _ => None,
        };
        self.cursor = next;
        self.input = next.map(|i| self.history[i].clone()).unwrap_or_default();
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    if ui.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Backtick)) {
        app.terminal.open = !app.terminal.open;
    }
    ui.add_space(6.0);
    let label = if app.terminal.open { tr("Masquer le terminal", "Hide terminal") } else { tr("Terminal", "Terminal") };
    if widgets::pill(ui, Some(Icon::Terminal), label, if app.terminal.open { CYAN } else { MUTED }, app.terminal.open)
        .on_hover_text(tr("Vos commandes, dans le dossier de travail (Ctrl+`)", "Your commands, in the workspace (Ctrl+`)"))
        .clicked()
    {
        app.terminal.open = !app.terminal.open;
    }
    if !app.terminal.open {
        return;
    }
    let mut run = None;
    Frame::new().fill(theme::rgba(8, 10, 18, 230)).corner_radius(12.0).inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ScrollArea::vertical().max_height(220.0).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
            let text = if app.terminal.output.is_empty() { tr("(aucune sortie)", "(no output)") } else { &app.terminal.output };
            ui.label(widgets::text(text, 12.0, SOFT).font(FontId::monospace(12.0)));
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(widgets::text(if app.terminal.running { "…" } else { "❯" }, 13.0, CYAN));
            let edit = ui.add_enabled(
                !app.terminal.running,
                TextEdit::singleline(&mut app.terminal.input)
                    .font(FontId::monospace(13.0))
                    .text_color(TEXT)
                    .frame(Frame::NONE)
                    .hint_text(tr("commande… (Entrée)", "command… (Enter)"))
                    .desired_width(f32::INFINITY),
            );
            if edit.has_focus() {
                if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
                    app.terminal.browse(true);
                } else if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
                    app.terminal.browse(false);
                }
            }
            if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                let cmd = app.terminal.input.trim().to_owned();
                if !cmd.is_empty() {
                    run = Some(cmd);
                }
                edit.request_focus();
            }
        });
        ui.label(widgets::text(
            tr("Vos commandes ne passent pas par le garde-fou de l'agent.", "Your commands bypass the agent's guard."),
            10.5,
            FAINT,
        ));
    });
    if let Some(cmd) = run {
        app.run_terminal(cmd);
    }
}

impl App {
    fn run_terminal(&mut self, cmd: String) {
        if cmd == "clear" || cmd == "cls" {
            self.terminal.output.clear();
            self.terminal.input.clear();
            return;
        }
        self.terminal.remember(&cmd);
        self.terminal.output.push_str(&format!("❯ {cmd}\n"));
        self.terminal.input.clear();
        self.terminal.running = true;
        let (dir, emit) = (self.env.workspace(), self.emit.clone());
        self.spawn(async move {
            let out = crate::tools::shell::execute(&dir, &cmd, TIMEOUT_SECS).await.unwrap_or_else(|e| format!("{} : {e:#}", tr("erreur", "error")));
            emit.send(Event::Terminal(out));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_browses_and_dedups() {
        let mut t = Terminal::default();
        t.remember("ls");
        t.remember("ls");
        t.remember("git status");
        assert_eq!(t.history.len(), 2);
        t.browse(true);
        assert_eq!(t.input, "git status");
        t.browse(true);
        assert_eq!(t.input, "ls");
        t.browse(false);
        assert_eq!(t.input, "git status");
        t.browse(false);
        assert_eq!(t.input, "");
    }

    #[test]
    fn scrollback_is_capped_on_a_char_boundary() {
        let mut t = Terminal::default();
        t.finished(&"é".repeat(SCROLLBACK));
        assert!(t.output.len() <= SCROLLBACK + 2);
        assert!(!t.running);
    }
}
