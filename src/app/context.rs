//! Request assembly in immutable order: [static system] → [history] → [current turn + <ctx>].
//! Append-only: everything before the current turn is byte-identical to the previous request → KV-cache hits,
//! and Claude's signed reasoning stays valid. Only compaction (a summary) rewrites the history, once.

use std::{fmt::Write as _, path::Path};

use serde_json::{Value, json};

use super::Env;
use crate::{
    domain::{
        chat::{Conversation, Message, Role},
        engine::{Engine, Wire},
        playbook::Playbook,
        route::Route,
        skill::Skill,
    },
    tools::{fs, schema},
};

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

/// What the active provider reads beyond the OpenAI-shaped core.
#[derive(Clone, Copy, Default)]
struct Target {
    vision: bool,
    /// Claude: signed reasoning and block layout (`false` = resend without the reasoning blocks).
    parts: bool,
    /// DeepSeek: `reasoning_content` of earlier answers.
    reasoning: bool,
}

/// `system` is built once per turn (`prompts::system`): a prompt that changes mid-turn (date, rules file)
/// would void Claude's signed reasoning and the prompt cache. `replay = false` drops Claude's reasoning blocks.
pub fn request(conv: &Conversation, env: &Env, system: &str, replay: bool) -> Value {
    let target = Target {
        vision: env.llm.engine.vision(),
        parts: replay && env.llm.wire() == Wire::Anthropic,
        reasoning: env.llm.engine == Engine::DeepSeek,
    };
    let mut messages = vec![json!({ "role": "system", "content": system })];
    let from = conv.summary.as_ref().map_or(0, |s| {
        messages.push(json!({ "role": "system", "content": format!("Summary of the earlier conversation:
{}", s.text) }));
        s.upto
    });
    messages.extend(conv.messages.iter().enumerate().skip(from).map(|(i, m)| {
        // Reasoning signed against an older prefix (other tools, system or history) is dropped as a leading run.
        let target = Target { parts: target.parts && i >= conv.thinking_from, ..target };
        wire(m, target)
    }));
    let tools = (!conv.toolset.is_empty()).then(|| schema::schemas(&conv.toolset));
    env.llm.body(&messages, tools)
}

fn wire(m: &Message, target: Target) -> Value {
    match m.role {
        Role::User => {
            let text =
                if m.preamble.is_empty() { m.content.clone() } else { format!("<ctx>\n{}\n</ctx>\n{}", m.preamble, m.content) };
            if target.vision && !m.images.is_empty() {
                // Claude rejects empty text blocks: an image-only turn sends images alone.
                let mut parts = Vec::new();
                if !text.is_empty() {
                    parts.push(json!({ "type": "text", "text": text }));
                }
                parts.extend(m.images.iter().map(|img| {
                    json!({ "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", img.media, img.data) } })
                }));
                json!({ "role": "user", "content": parts })
            } else {
                // Image-only turn on a text engine: keep a non-empty user message.
                let text = if text.is_empty() && !m.images.is_empty() { "[image]".to_owned() } else { text };
                json!({ "role": "user", "content": text })
            }
        }
        Role::Assistant => {
            let mut v = json!({ "role": "assistant", "content": m.content });
            if target.reasoning && !m.reasoning.is_empty() {
                v["reasoning_content"] = json!(m.reasoning);
            }
            if !m.tool_calls.is_empty() {
                v["tool_calls"] = m
                    .tool_calls
                    .iter()
                    .map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.arguments } }))
                    .collect();
            }
            if target.parts && !m.parts.is_empty() {
                v["parts"] = serde_json::to_value(&m.parts).unwrap_or_default();
            }
            v
        }
        Role::Tool => {
            json!({ "role": "tool", "tool_call_id": m.tool_call_id, "content": m.content })
        }
    }
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

    fn screenshot() -> crate::domain::chat::Image {
        crate::domain::chat::Image { media: "image/png".into(), data: "QUJD".into() }
    }

    #[test]
    fn an_image_only_turn_sends_no_empty_text_block() {
        let m = Message::user_with(String::new(), vec![screenshot()]);
        let w = wire(&m, Target { vision: true, ..Target::default() });
        let parts = w["content"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "image_url");
    }

    #[test]
    fn a_text_engine_gets_a_placeholder_for_an_image_only_turn() {
        let m = Message::user_with(String::new(), vec![screenshot()]);
        let w = wire(&m, Target::default());
        assert_eq!(w["content"], "[image]");
    }

    #[test]
    fn a_claude_tool_loop_replays_its_signed_reasoning() {
        use crate::domain::{
            auth::AuthMode,
            chat::{Part, ToolCall},
            settings::Settings,
        };
        let settings = Settings {
            engine: Engine::Claude,
            claude_auth: AuthMode::ApiKey,
            claude_key: "k".into(),
            claude_model: "claude-opus-5-5".into(),
            ..Settings::default()
        };
        let web = std::sync::Arc::new(crate::tools::webindex::WebIndex::open(&std::env::temp_dir().join("omega-ctx-web")));
        let env = Env::new(settings, web).unwrap();
        let mut conv = Conversation::default();
        conv.push(Message::user("cherche".into()));
        let call = ToolCall { id: "toolu_1".into(), name: "grep".into(), arguments: "{}".into() };
        let parts = vec![Part::Thinking { thinking: String::new(), signature: "sig".into() }, Part::ToolUse { call: 0 }];
        conv.push(Message { parts, ..Message::assistant(String::new(), String::new(), vec![call]) });
        conv.push(Message::tool("toolu_1".into(), "1 hit".into()));
        let body = request(&conv, &env, "SYS", true);
        assert_eq!(body["messages"][1]["content"][0], json!({ "type": "thinking", "thinking": "", "signature": "sig" }));
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(body["thinking"]["type"], "adaptive");
        let stripped = request(&conv, &env, "SYS", false);
        assert_eq!(stripped["messages"][1]["content"][0]["type"], "tool_use", "repli sans les blocs de pensée");
    }

    #[test]
    fn provider_specific_fields_only_reach_the_provider_that_reads_them() {
        use crate::domain::chat::{Part, ToolCall};
        let call = ToolCall { id: "c1".into(), name: "grep".into(), arguments: "{}".into() };
        let m = Message {
            parts: vec![Part::Thinking { thinking: String::new(), signature: "s".into() }, Part::ToolUse { call: 0 }],
            ..Message::assistant("ok".into(), "pensée".into(), vec![call])
        };
        let plain = wire(&m, Target::default());
        assert!(plain.get("parts").is_none() && plain.get("reasoning_content").is_none());
        let all = wire(&m, Target { parts: true, reasoning: true, ..Target::default() });
        assert_eq!(all["parts"][0]["type"], "thinking");
        assert_eq!(all["reasoning_content"], "pensée");
    }

    #[test]
    fn a_conversation_saved_with_gemini_signatures_still_loads() {
        let call: crate::domain::chat::ToolCall =
            serde_json::from_str(r#"{"id":"c1","name":"grep","arguments":"{}","signature":"sig"}"#).expect("champ retiré toléré");
        assert_eq!(call.name, "grep");
    }
}
