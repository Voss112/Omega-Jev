//! Interface language: one process-wide switch, read by `tr` wherever a label is drawn.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::domain::settings::UiLang;

static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn set(lang: UiLang) {
    ENGLISH.store(lang == UiLang::En, Ordering::Relaxed);
}

/// The label in the selected language.
pub fn tr(fr: &'static str, en: &'static str) -> &'static str {
    if ENGLISH.load(Ordering::Relaxed) { en } else { fr }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_language() {
        set(UiLang::En);
        assert_eq!(tr("Envoyer", "Send"), "Send");
        set(UiLang::Fr);
        assert_eq!(tr("Envoyer", "Send"), "Envoyer");
    }
}
