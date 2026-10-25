//! Resume after the Claude plan limit: the turn waits, the 30 s quota refresh watches the %, and once the
//! window that hit its cap has reset the conversation continues by itself.

use std::collections::HashMap;

use uuid::Uuid;

use super::App;
use crate::domain::{
    auth::{Provider, Window},
    engine::Engine,
};

/// A window at or above this share is the one that stopped the turn.
const CAPPED: f32 = 0.95;
/// It counts as reset once it falls below this share.
const RESET: f32 = 0.10;

/// Conversations waiting for the Claude quota, with the windows that were capped when they stopped.
#[derive(Default)]
pub struct Waiting(HashMap<Uuid, Vec<String>>);

/// The error a Claude turn ended with is a plan limit (not a bug, not a network failure).
pub fn plan_limit(error: &str) -> bool {
    let lower = error.to_lowercase();
    ["429", "rate_limit", "usage limit", "quota", "limit reached", "exhausted"].iter().any(|m| lower.contains(m))
}

fn capped(windows: &[Window]) -> Vec<String> {
    windows.iter().filter(|w| w.fraction >= CAPPED).map(|w| w.label.clone()).collect()
}

/// Every window that was capped has reset; with none known, every window must be clear of its cap.
fn released(was: &[String], now: &[Window]) -> bool {
    if was.is_empty() {
        return !now.is_empty() && now.iter().all(|w| w.fraction < CAPPED);
    }
    was.iter().all(|label| now.iter().find(|w| &w.label == label).is_some_and(|w| w.fraction < RESET))
}

impl App {
    /// End of a failed turn: a Claude plan limit puts the conversation on hold instead of failing it.
    pub(super) fn hold_on_quota(&mut self, id: Uuid, engine: Engine, error: &str) -> bool {
        let subscription = self.settings.auth_mode(Engine::Claude) == crate::domain::auth::AuthMode::Subscription;
        if !self.settings.resume_on_quota || engine != Engine::Claude || !subscription || !plan_limit(error) {
            return false;
        }
        let windows = self.plan_usage.get(&Provider::Claude).map(|(w, _)| capped(w)).unwrap_or_default();
        self.waiting.0.insert(id, windows);
        // Read the quota again now rather than in up to 30 s.
        self.usage_polled.remove(&Provider::Claude);
        // A throttled token renewal is not the plan limit: the next quota reading proves the token works again.
        self.notify(if crate::infra::auth::renewal_throttled(error) {
            super::i18n::tr(
                "Renouvellement du jeton Claude limité (429, pas votre quota) : reprise automatique dès qu'il passe",
                "Claude token renewal rate-limited (429, not your quota): resuming automatically once it goes through",
            )
        } else {
            super::i18n::tr(
                "Limite Claude atteinte : reprise automatique dès que le quota retombe",
                "Claude limit reached: resuming automatically once the quota drops",
            )
        });
        true
    }

    /// Called with each fresh Claude reading: resumes the conversations whose capped window has reset.
    pub(super) fn resume_after_quota(&mut self, windows: &[Window]) {
        let ready: Vec<Uuid> = self.waiting.0.iter().filter(|(_, was)| released(was, windows)).map(|(id, _)| *id).collect();
        for id in ready {
            self.waiting.0.remove(&id);
            if self.live.contains_key(&id) {
                continue;
            }
            self.errors.remove(&id);
            let text = super::i18n::tr(
                "Le quota est revenu : reprends exactement où tu t'es arrêté et termine la tâche.",
                "Quota is back: resume exactly where you stopped and finish the task.",
            );
            self.submit(Some(id), text.to_owned(), Vec::new());
        }
    }

    pub(super) fn cancel_wait(&mut self, id: Uuid) {
        self.waiting.0.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(label: &str, fraction: f32) -> Window {
        Window { label: label.into(), fraction, ..Window::default() }
    }

    #[test]
    fn only_plan_limits_hold_the_turn() {
        assert!(plan_limit("Claude 429: rate_limit_error — usage limit reached"));
        assert!(!plan_limit("connexion refusée"));
    }

    #[test]
    fn resumes_once_the_capped_window_resets() {
        let was = capped(&[w("5 h", 1.0), w("7 jours", 0.7)]);
        assert_eq!(was, ["5 h"]);
        assert!(!released(&was, &[w("5 h", 0.98), w("7 jours", 0.7)]));
        assert!(!released(&was, &[w("5 h", 0.4), w("7 jours", 0.7)]), "pas encore retombé");
        assert!(released(&was, &[w("5 h", 0.0), w("7 jours", 0.71)]));
    }

    #[test]
    fn unknown_cap_waits_for_every_window_to_clear() {
        assert!(!released(&[], &[]));
        assert!(!released(&[], &[w("5 h", 0.99)]));
        assert!(released(&[], &[w("5 h", 0.2), w("7 jours", 0.5)]));
    }
}
