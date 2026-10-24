//! Messages the user forces into a running turn: the agent reads them after its next tool step.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use uuid::Uuid;

/// Shared between the UI (writer) and the agent loop (reader), keyed by conversation.
#[derive(Clone, Default)]
pub struct Inbox(Arc<Mutex<HashMap<Uuid, Vec<String>>>>);

impl Inbox {
    pub fn push(&self, conv: Uuid, text: String) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).entry(conv).or_default().push(text);
    }

    /// Everything waiting for `conv`, oldest first; the box is left empty.
    pub fn take(&self, conv: Uuid) -> Vec<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).remove(&conv).unwrap_or_default()
    }

    /// Note appended to a tool result so the model reads the forced messages mid-turn.
    pub fn note(&self, conv: Uuid) -> Option<String> {
        let texts = self.take(conv);
        if texts.is_empty() {
            return None;
        }
        let body = texts.iter().map(|t| format!("> {}", t.replace('\n', "\n> "))).collect::<Vec<_>>().join("\n\n");
        Some(format!("[message de l'utilisateur reçu pendant le travail — à prendre en compte maintenant]\n{body}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_messages_are_read_once_in_order() {
        let inbox = Inbox::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        inbox.push(a, "un".into());
        inbox.push(a, "deux\nlignes".into());
        inbox.push(b, "autre".into());
        let note = inbox.note(a).unwrap();
        assert!(note.contains("> un\n\n> deux\n> lignes"));
        assert!(inbox.note(a).is_none(), "lu une seule fois");
        assert_eq!(inbox.take(b), vec!["autre".to_owned()]);
    }
}
