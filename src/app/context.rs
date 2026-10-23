//! Request assembly in immutable order: [static system] → [history] → [current turn + <ctx>].
//! Everything before the current turn is byte-identical to the previous request → KV-cache hits.

use std::{fmt::Write as _, path::Path};

use serde_json::{Value, json};

use super::{Env, prompts};
use crate::{
    domain::{
        chat::{Conversation, Message, Role},
        playbook::Playbook,
        route::Route,
        skill::Skill,
    },
    tools::{compress::clip, fs, schema},
};

/// Tool outputs older than this many user turns are compacted.
const KEEP_TURNS: usize = 3;
const COMPACT_CHARS: usize = 600;

/// `loaded`: playbook ids already present earlier in the history (not re-sent).
pub fn preamble(route: &Route, root: &Path, first_turn: bool, loaded: &[&str]) -> String {
    let mut p = format!("[skill:{}] {}", route.skill.id(), route.skill.prompt());
    if let Some(book) = route.playbook.as_deref().and_then(Playbook::find) {
        if loaded.contains(&book.id) {
            let _ = write!(p, "\n[playbook:{}] Follow it again (full text earlier in this conversation).", book.id);
        } else {
            let _ = write!(p, "\n[playbook:{}]\n{}", book.id, book.body());
        }
    }
    if route.risky() {
        p.push_str("\n[guard] Security-sensitive: validate inputs, no unbounded shell, never print secrets.");
    }
    if route.skill == Skill::Security {
        p.push_str(
            "\n[audit] Six phases: reconnaissance → hunting par couverture → validation des candidats → findings.json \
             (verdicts confirmed/needs_validation/rejected) → vérification indépendante → REPORT.md. \
             Un record = une preuve fichier:ligne. Zéro faux positif : ce qui n'est pas démontré est SUSPECT.",
        );
    }
    if route.needs_apps() {
        p.push_str(
            "
[apps] Real software via cli_anything: search → info (read usage) → install once → run (JSON).",
        );
    }
    if route.wide_impact() {
        p.push_str("\n[impact] Public API change likely: run blast_radius first, update every caller.");
    }
    if first_turn && route.needs_code() {
        p.push_str("\n[workspace]\n");
        p.push_str(&fs::tree(root, 2, 60));
    }
    p
}

pub fn request(conv: &Conversation, env: &Env) -> Value {
    let cutoff = compaction_cutoff(conv);
    let mut messages = vec![json!({ "role": "system", "content": prompts::system(&env.settings, &env.workspace()) })];
    let from = conv.summary.as_ref().map_or(0, |s| {
        messages.push(json!({ "role": "system", "content": format!("Summary of the earlier conversation:
{}", s.text) }));
        s.upto
    });
    messages.extend(conv.messages.iter().enumerate().skip(from).map(|(i, m)| wire(m, i < cutoff)));
    let tools = (!conv.toolset.is_empty()).then(|| schema::schemas(&conv.toolset));
    env.llm.body(&messages, tools)
}

fn wire(m: &Message, compact: bool) -> Value {
    match m.role {
        Role::User if m.preamble.is_empty() => json!({ "role": "user", "content": m.content }),
        Role::User => json!({ "role": "user", "content": format!("<ctx>\n{}\n</ctx>\n{}", m.preamble, m.content) }),
        Role::Assistant => {
            let mut v = json!({ "role": "assistant", "content": m.content });
            if !m.reasoning.is_empty() {
                v["reasoning_content"] = json!(m.reasoning);
            }
            if !m.tool_calls.is_empty() {
                v["tool_calls"] = m
                    .tool_calls
                    .iter()
                    .map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.arguments } }))
                    .collect();
            }
            v
        }
        Role::Tool => {
            let content = if compact { clip(&m.content, COMPACT_CHARS) } else { m.content.clone() };
            json!({ "role": "tool", "tool_call_id": m.tool_call_id, "content": content })
        }
    }
}

/// Index of the KEEP_TURNS-th last user message; tool outputs before it are compacted.
fn compaction_cutoff(conv: &Conversation) -> usize {
    conv.messages.iter().enumerate().filter(|(_, m)| m.role == Role::User).map(|(i, _)| i).rev().nth(KEEP_TURNS - 1).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::route::Source;

    fn security_route() -> Route {
        Route {
            skill: Skill::Security,
            source: Source::Local,
            confidence: 1.0,
            web: 0.0,
            code: 0.6,
            risk: 1.0,
            blast: 0.0,
            apps: 0.0,
            playbook: Some("security-audit".into()),
        }
    }

    #[test]
    fn security_turns_carry_the_audit_hint() {
        let p = preamble(&security_route(), std::path::Path::new("."), false, &[]);
        assert!(p.contains("[playbook:security-audit]"));
        assert!(p.contains("[audit]") && p.contains("findings.json"));
        assert!(p.contains("[guard]"));
    }

    #[test]
    fn an_already_loaded_playbook_is_not_resent() {
        let p = preamble(&security_route(), std::path::Path::new("."), false, &["security-audit"]);
        assert!(p.contains("Follow it again"));
        assert_eq!(p.matches("security-audit").count(), 1, "le corps ne doit pas être renvoyé");
    }
}
