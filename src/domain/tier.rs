//! Claude model tier per turn: Jev's difficulty picks Haiku, Sonnet or the user's model, never above the user's.

/// Families ordered cheapest → most capable.
const FAMILIES: [&str; 3] = ["haiku", "sonnet", "opus"];

/// Below this difficulty a turn runs on Haiku (display a text, rename, small style fix).
const HAIKU_BELOW: f32 = 0.3;
/// Below this one, on Sonnet; above, on the user's model.
const SONNET_BELOW: f32 = 0.65;
/// Past this prompt size a switch writes the whole history into a cold cache: only worth it when the
/// cheaper model's cache is already warm (it answered the previous turn).
pub const COLD_SWITCH_MAX: u64 = 40_000;

/// `claude-<family>-<version>` → (family rank, the id with `{family}` in place of the family).
fn split(model: &str) -> Option<(usize, String)> {
    let parts: Vec<&str> = model.split('-').collect();
    let at = parts.iter().position(|p| FAMILIES.contains(p))?;
    let rank = FAMILIES.iter().position(|f| *f == parts[at])?;
    let mut template = parts.clone();
    template[at] = "{family}";
    Some((rank, template.join("-")))
}

/// Model for a turn of difficulty `d` (0..1) when the user chose `chosen`; `None` = keep `chosen`.
/// Same generation as `chosen` (opus-5-5 → sonnet-5-5 → haiku-5-5); unknown families are never changed.
pub fn pick(chosen: &str, d: f32) -> Option<String> {
    let (top, template) = split(chosen)?;
    let wanted = if d < HAIKU_BELOW {
        0
    } else if d < SONNET_BELOW {
        1
    } else {
        top
    };
    let rank = wanted.min(top);
    (rank != top).then(|| template.replace("{family}", FAMILIES[rank]))
}

/// A cheaper model is worth it unless its cache is cold and the history large (`context` prompt tokens).
pub fn switch_pays(candidate: &str, previous_model: Option<&str>, context: u64) -> bool {
    context <= COLD_SWITCH_MAX || previous_model == Some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easy_turns_go_down_a_tier_in_the_same_generation() {
        assert_eq!(pick("claude-opus-5-5", 0.1).as_deref(), Some("claude-haiku-5-5"));
        assert_eq!(pick("claude-opus-5-5", 0.5).as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(pick("claude-opus-5-5", 0.9), None);
        assert_eq!(pick("claude-sonnet-4-5", 0.2).as_deref(), Some("claude-haiku-4-5"));
    }

    #[test]
    fn never_above_the_users_choice_nor_on_unknown_families() {
        assert_eq!(pick("claude-sonnet-5-5", 0.95), None, "Sonnet choisi : jamais Opus");
        assert_eq!(pick("claude-haiku-5-5", 0.1), None);
        assert_eq!(pick("claude-fable-5-1", 0.1), None, "famille inconnue : inchangée");
        assert_eq!(pick("custom-model", 0.1), None);
    }

    #[test]
    fn a_large_history_only_switches_to_a_warm_cache() {
        assert!(switch_pays("claude-haiku-5-5", None, 10_000));
        assert!(!switch_pays("claude-haiku-5-5", Some("claude-opus-5-5"), 90_000));
        assert!(switch_pays("claude-haiku-5-5", Some("claude-haiku-5-5"), 90_000));
    }
}
