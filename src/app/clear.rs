//! Mid-turn context clearing. Compaction only runs between turns, so one long tool loop grows the context without
//! bound and every step re-reads all of it. Past the context budget, the oldest bulky tool results are replaced on
//! the wire by a one-line stub (the UI keeps the text). One sizeable batch at a time: each batch costs one cache
//! break, so it frees enough to fall well under the budget and is skipped when the gain would be marginal.

use crate::{
    domain::chat::{Conversation, Message, Role},
    tools::compress::clip,
};

/// Most recent tool results, always kept verbatim: the model is still working with them.
const KEEP_RECENT: usize = 6;
/// Smaller results cost little more than their stub.
const MIN_CHARS: usize = 1_200;
/// After a batch, the context should sit at this share of the budget.
const TARGET: f64 = 0.6;
/// A batch freeing less than this share of the budget is not worth its cache break.
const MIN_GAIN: f64 = 0.15;

/// Indices of the tool results to clear now (oldest first), or none.
/// `ratio`: tokens per byte measured on this conversation.
pub fn plan(conv: &Conversation, budget: u64, ratio: f64) -> Vec<usize> {
    if budget == 0 || conv.context < budget {
        return Vec::new();
    }
    let from = conv.summary.as_ref().map_or(0, |s| s.upto).min(conv.messages.len());
    let results: Vec<usize> = (from..conv.messages.len()).filter(|&i| conv.messages[i].role == Role::Tool).collect();
    let older = &results[..results.len().saturating_sub(KEEP_RECENT)];
    let need = conv.context as f64 - budget as f64 * TARGET;
    let mut freed = 0.0;
    let mut picked = Vec::new();
    for &i in older {
        let m = &conv.messages[i];
        if m.cleared || m.content.len() < MIN_CHARS {
            continue;
        }
        freed += m.content.len().saturating_sub(stub(m).len()) as f64 * ratio;
        picked.push(i);
        if freed >= need {
            break;
        }
    }
    if freed < budget as f64 * MIN_GAIN { Vec::new() } else { picked }
}

pub fn apply(conv: &mut Conversation, indices: &[usize]) {
    for &i in indices {
        if let Some(m) = conv.messages.get_mut(i) {
            m.cleared = true;
        }
    }
}

/// What the model sees instead of a cleared result: its first line, so it knows what to call again.
pub fn stub(m: &Message) -> String {
    let head = m.content.lines().next().unwrap_or_default();
    format!("[cleared to save context: « {} » ({} chars). Call the tool again if you need it.]", clip(head, 100), m.content.len())
}

/// Bytes cleared results no longer put on the wire.
pub fn saved_bytes(visible: &[Message]) -> u64 {
    visible.iter().filter(|m| m.cleared).map(|m| m.content.len().saturating_sub(stub(m).len()) as u64).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::chat::Summary;

    /// A user turn then `n` tool results of `size` bytes each.
    fn turn(n: usize, size: usize) -> Conversation {
        let mut conv = Conversation::default();
        conv.push(Message::user("go".into()));
        for k in 0..n {
            conv.push(Message::assistant(String::new(), String::new(), Vec::new()));
            conv.push(Message::tool(format!("c{k}"), format!("src/f{k}.rs (200 lines)\n{}", "x".repeat(size))));
        }
        conv
    }

    #[test]
    fn under_budget_nothing_is_cleared() {
        let conv = Conversation { context: 50_000, ..turn(20, 40_000) };
        assert!(plan(&conv, 100_000, 0.25).is_empty());
        assert!(plan(&Conversation { context: 500_000, ..turn(20, 40_000) }, 0, 0.25).is_empty(), "budget 0 = désactivé");
    }

    #[test]
    fn past_budget_the_oldest_results_go_first_and_the_recent_ones_stay() {
        let conv = Conversation { context: 150_000, ..turn(20, 40_000) };
        let picked = plan(&conv, 100_000, 0.25);
        // Need 150k − 60k = 90k tokens; each result frees ~10k → 10 results, the oldest ones.
        assert_eq!(picked.len(), 10, "{picked:?}");
        assert_eq!(picked[0], 2, "le plus ancien résultat d'abord");
        let tools: Vec<usize> = (0..conv.messages.len()).filter(|&i| conv.messages[i].role == Role::Tool).collect();
        assert!(tools[tools.len() - KEEP_RECENT..].iter().all(|i| !picked.contains(i)), "les 6 derniers restent");
    }

    #[test]
    fn a_marginal_gain_is_not_worth_a_cache_break() {
        // Only small results to clear: well below 15 % of the budget.
        let conv = Conversation { context: 150_000, ..turn(20, 1_500) };
        assert!(plan(&conv, 100_000, 0.25).is_empty());
    }

    #[test]
    fn cleared_or_summarized_results_are_not_picked_again() {
        let mut conv = Conversation { context: 150_000, ..turn(20, 40_000) };
        let first = plan(&conv, 100_000, 0.25);
        apply(&mut conv, &first);
        assert!(plan(&conv, 100_000, 0.25).iter().all(|i| !first.contains(i)));
        let summarized = Conversation { summary: Some(Summary { upto: 30, text: "s".into() }), ..conv };
        assert!(plan(&summarized, 100_000, 0.25).iter().all(|&i| i >= 30));
    }

    #[test]
    fn the_stub_names_what_was_cleared_and_the_savings_are_counted() {
        let mut conv = turn(1, 5_000);
        apply(&mut conv, &[2]);
        let s = stub(&conv.messages[2]);
        assert!(s.contains("src/f0.rs (200 lines)") && s.contains("chars"), "{s}");
        assert_eq!(saved_bytes(&conv.messages), (conv.messages[2].content.len() - s.len()) as u64);
    }
}
