//! Interface language: one process-wide switch, read by `tr` wherever a label is drawn.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::domain::settings::UiLang;

static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn set(lang: UiLang) {
    ENGLISH.store(lang == UiLang::En, Ordering::Relaxed);
}

/// English interface selected (for texts built with `format!`).
pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// The label in the selected language.
pub fn tr(fr: &'static str, en: &'static str) -> &'static str {
    pick(english(), fr, en)
}

fn pick(english: bool, fr: &'static str, en: &'static str) -> &'static str {
    if english { en } else { fr }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests run in parallel and assert French texts: the process-wide switch is never flipped here.
    #[test]
    fn switches_language() {
        assert_eq!(pick(true, "Envoyer", "Send"), "Send");
        assert_eq!(pick(false, "Envoyer", "Send"), "Envoyer");
        assert_eq!(tr("Envoyer", "Send"), "Envoyer", "French by default");
    }
}
