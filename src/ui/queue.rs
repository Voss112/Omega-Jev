//! Prompt queue: prompts typed during a running turn wait their turn, or are forced into it.

use eframe::egui::{FontId, Ui};
use uuid::Uuid;

use super::{
    App,
    i18n::tr,
    icons::Icon,
    theme::{ACCENT_HI, AMBER, FAINT, SOFT},
    widgets,
};
use crate::domain::chat::Image;

/// Queued prompts shown per conversation; beyond this the oldest wait silently.
const SHOWN: usize = 5;
/// Prompts one conversation may hold in its queue.
const MAX_QUEUED: usize = 20;

pub struct Queued {
    pub text: String,
    pub images: Vec<Image>,
}

enum Action {
    Force(usize),
    Send(usize),
    Remove(usize),
}

impl App {
    /// Composer content goes to the back of the current conversation's queue.
    pub(super) fn enqueue(&mut self, text: String) {
        let Some(id) = self.current else { return };
        let queue = self.queue.entry(id).or_default();
        if queue.len() >= MAX_QUEUED {
            self.notify(format!("File d'attente pleine ({MAX_QUEUED} prompts)"));
            return;
        }
        queue.push_back(Queued { text: super::docs::inline(&text, &self.docs), images: std::mem::take(&mut self.shots) });
        self.docs.clear();
        self.draft.clear();
        self.notify(tr("Ajouté à la file : envoyé à la fin du tour en cours", "Queued: sent when the current turn ends"));
    }

    /// Composer content is read by the running agent after its next tool step.
    pub(super) fn force_draft(&mut self) {
        let text = super::docs::inline(self.draft.trim(), &self.docs);
        let Some(id) = self.current.filter(|_| !text.is_empty() && self.busy()) else { return };
        self.docs.clear();
        self.draft.clear();
        self.force(id, text);
    }

    fn force(&mut self, id: Uuid, text: String) {
        self.env.inbox.push(id, text);
        self.notify(tr("Transmis à l'agent : il le lit après sa prochaine action", "Passed to the agent: read after its next action"));
    }

    /// End of a turn: unread forced messages come first, then the queue resumes (paused after an error).
    pub(super) fn turn_ended(&mut self, id: Uuid, failed: bool) {
        let unread = self.env.inbox.take(id);
        let queue = self.queue.entry(id).or_default();
        for text in unread.into_iter().rev() {
            queue.push_front(Queued { text, images: Vec::new() });
        }
        if failed {
            if !queue.is_empty() {
                self.notify(tr("Tour interrompu : la file d'attente est en pause (Envoyer pour reprendre)", "Turn interrupted: the queue is paused (Send to resume)"));
            }
            return;
        }
        self.send_queued(id, 0);
    }

    fn send_queued(&mut self, id: Uuid, index: usize) {
        if self.live.contains_key(&id) || self.vpn_blocks() {
            return;
        }
        let Some(item) = self.queue.get_mut(&id).and_then(|q| q.remove(index)) else { return };
        self.submit(Some(id), item.text, item.images);
    }

    fn act(&mut self, id: Uuid, action: Action) {
        match action {
            Action::Force(i) => {
                if let Some(item) = self.queue.get_mut(&id).and_then(|q| q.remove(i)) {
                    self.force(id, item.text);
                }
            }
            Action::Send(i) => self.send_queued(id, i),
            Action::Remove(i) => {
                self.queue.get_mut(&id).map(|q| q.remove(i));
            }
        }
    }
}

/// Pending prompts of the current conversation, above the composer.
pub fn strip(app: &mut App, ui: &mut Ui) {
    let Some(id) = app.current else { return };
    let busy = app.busy();
    let Some(queue) = app.queue.get(&id).filter(|q| !q.is_empty()) else { return };
    let mut action = None;
    ui.label(widgets::text(format!("{} · {}", tr("File d'attente", "Queue"), queue.len()), 11.5, AMBER));
    for (i, item) in queue.iter().enumerate().take(SHOWN) {
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 190.0).max(80.0);
            ui.allocate_ui(eframe::egui::vec2(width, 22.0), |ui| {
                let label = if item.images.is_empty() {
                    item.text.clone()
                } else {
                    format!("🖼 {} · {}", item.images.len(), item.text)
                };
                widgets::truncated(ui, &label, FontId::proportional(12.5), SOFT);
            });
            if busy {
                let tip = "L'agent lit ce message pendant qu'il travaille (après sa prochaine action)";
                if widgets::pill(ui, Some(Icon::Bolt), tr("Forcer", "Force"), ACCENT_HI, false).on_hover_text(tip).clicked() {
                    action = Some(Action::Force(i));
                }
            } else if widgets::pill(ui, Some(Icon::ArrowUp), tr("Envoyer", "Send"), ACCENT_HI, false).clicked() {
                action = Some(Action::Send(i));
            }
            if widgets::icon_button(ui, Icon::Close, "Retirer de la file").clicked() {
                action = Some(Action::Remove(i));
            }
        });
    }
    if queue.len() > SHOWN {
        ui.label(widgets::text(format!("… et {} autre(s)", queue.len() - SHOWN), 11.0, FAINT));
    }
    ui.add_space(6.0);
    if let Some(action) = action {
        app.act(id, action);
    }
}
