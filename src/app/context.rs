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
        tool::ToolKind,
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
    // Without this, a model that sees tools described in the system prompt writes its call markup as text.
    if route.tools().is_empty() {
        p.push_str("
[no-tools] No tool is available this turn: answer directly in text, never write tool-call markup.");
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
    /// Gemini: thought signatures of tool calls, in the wire's own field.
    signatures: Option<Wire>,
}

/// `system` is built once per turn (`prompts::system`): a prompt that changes mid-turn (date, rules file)
/// would void Claude's signed reasoning and the prompt cache. `replay = false` drops Claude's reasoning blocks.
pub fn request(conv: &Conversation, env: &Env, system: &str, replay: bool) -> Value {
    let target = Target {
        vision: env.llm.engine.vision(),
        parts: replay && env.llm.wire() == Wire::Anthropic,
        reasoning: env.llm.engine == Engine::DeepSeek,
        signatures: (env.llm.engine == Engine::Gemini).then(|| env.llm.wire()),
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
    let visible = &conv.messages[from.min(conv.messages.len())..];
    if !env.llm.deferred_tools() {
        if target.vision {
            attach_screenshot(&mut messages, visible);
        }
        let tools = (!conv.toolset.is_empty()).then(|| schema::schemas(&conv.toolset));
        return env.llm.body(&messages, tools);
    }
    let mut messages = surface_additions(messages, &conv.messages[from..]);
    if target.vision {
        attach_screenshot(&mut messages, visible);
    }
    env.llm.body(&messages, Some(schema::deferred(ToolKind::READ)))
}

/// Adds a `tools` entry (a `tool_addition` system message on the Claude wire) after each user message that added
/// tools. Such a message must precede an assistant turn or end the request: an addition on an interrupted turn
/// waits for the next user message. `visible` are the conversation messages ending `messages`.
fn surface_additions(messages: Vec<Value>, visible: &[Message]) -> Vec<Value> {
    let head = messages.len() - visible.len();
    let mut out = Vec::with_capacity(messages.len() + 2);
    let mut pending: Vec<ToolKind> = Vec::new();
    for (k, v) in messages.into_iter().enumerate() {
        out.push(v);
        let Some(i) = k.checked_sub(head) else { continue };
        let m = &visible[i];
        for t in &m.tools_added {
            if !pending.contains(t) {
                pending.push(*t);
            }
        }
        let next_is_answer = visible.get(i + 1).is_none_or(|n| n.role == Role::Assistant);
        if m.role == Role::User && next_is_answer && !pending.is_empty() {
            let add: Vec<&str> = pending.drain(..).map(ToolKind::name).collect();
            out.push(json!({ "role": "tools", "add": add }));
        }
    }
    out
}

/// Tools of the conversation that no visible user turn has offered yet (a summary hides earlier additions).
/// Deferred mode only: the always-offered base is not counted.
pub fn tool_additions(conv: &Conversation, from: usize) -> Vec<ToolKind> {
    let offered: Vec<ToolKind> = conv.messages[from.min(conv.messages.len())..].iter().flat_map(|m| m.tools_added.iter().copied()).collect();
    conv.toolset.iter().copied().filter(|k| !ToolKind::READ.contains(k) && !offered.contains(k)).collect()
}

/// Only the newest screenshot travels (older ones would cost thousands of tokens each): it follows the run of tool
/// results that holds it, as a user message every vision wire accepts.
fn attach_screenshot(messages: &mut Vec<Value>, visible: &[Message]) {
    let Some(shot) = visible.iter().rev().find(|m| m.role == Role::Tool && !m.images.is_empty()) else { return };
    let Some(at) = messages.iter().rposition(|v| v["role"] == "tool" && v["tool_call_id"].as_str() == shot.tool_call_id.as_deref())
    else {
        return;
    };
    let end = messages[at..].iter().position(|v| v["role"] != "tool").map_or(messages.len(), |n| at + n);
    let mut parts = vec![json!({ "type": "text", "text": "[capture d'écran du dernier appel computer]" })];
    parts.extend(shot.images.iter().map(|img| {
        json!({ "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", img.media, img.data) } })
    }));
    messages.insert(end, json!({ "role": "user", "content": parts }));
}

/// Google's placeholder for a function call that has no thought signature of its own.
const GEMINI_NO_SIGNATURE: &str = "skip_thought_signature_validator";

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
                    .map(|c| {
                        let mut call =
                            json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.arguments } });
                        if let Some(wire) = target.signatures {
                            // Calls written by another engine carry no signature: Gemini 3 accepts this documented
                            // placeholder instead of rejecting the whole history.
                            let sig = if c.signature.is_empty() { GEMINI_NO_SIGNATURE } else { c.signature.as_str() };
                            if wire == Wire::Gemini {
                                call["signature"] = json!(sig);
                            } else {
                                call["extra_content"] = json!({ "google": { "thought_signature": sig } });
                            }
                        }
                        call
                    })
                    .collect();
            }
            if target.parts && !m.parts.is_empty() {
                v["parts"] = serde_json::to_value(&m.parts).unwrap_or_default();
            }
            v
        }
        Role::Tool => {
            let content = if m.cleared { super::clear::stub(m) } else { m.content.clone() };
            json!({ "role": "tool", "tool_call_id": m.tool_call_id, "content": content })
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
            effort: None,
            simple: 0.0,
            routine: 0.0,
            design: 0.0,
        }
    }

    #[test]
    fn only_the_newest_screenshot_follows_its_tool_results() {
        use crate::domain::chat::{Image, ToolCall};
        let shot = |id: &str, data: &str| Message { images: vec![Image::new("image/png", data.into())], ..Message::tool(id.into(), "ok".into()) };
        let call = |id: &str| ToolCall { id: id.into(), name: "computer".into(), arguments: "{}".into(), ..ToolCall::default() };
        let visible = vec![
            Message::assistant(String::new(), String::new(), vec![call("a")]),
            shot("a", "OLD"),
            Message::assistant(String::new(), String::new(), vec![call("b"), call("c")]),
            shot("b", "NEW"),
            Message::tool("c".into(), "x".into()),
        ];
        let mut messages: Vec<Value> = visible.iter().map(|m| wire(m, Target::default())).collect();
        attach_screenshot(&mut messages, &visible);
        assert_eq!(messages.len(), 6);
        let last = &messages[5];
        assert_eq!(last["role"], "user", "après tout le lot de résultats d'outils");
        let text = last.to_string();
        assert!(text.contains("base64,NEW") && !text.contains("OLD"), "{text}");
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
        crate::domain::chat::Image::new("image/png", "QUJD".into())
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
        let call = ToolCall { id: "toolu_1".into(), name: "grep".into(), arguments: "{}".into(), ..ToolCall::default() };
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
        let call = ToolCall { id: "c1".into(), name: "grep".into(), arguments: "{}".into(), ..ToolCall::default() };
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
    fn gemini_gets_its_signatures_and_others_never_see_them() {
        use crate::domain::chat::ToolCall;
        let signed = ToolCall { id: "c1".into(), name: "grep".into(), arguments: "{}".into(), signature: "s".into() };
        let foreign = ToolCall { id: "toolu_2".into(), name: "grep".into(), arguments: "{}".into(), ..ToolCall::default() };
        let m = Message::assistant(String::new(), String::new(), vec![signed, foreign]);
        let code_assist = wire(&m, Target { signatures: Some(Wire::Gemini), ..Target::default() });
        assert_eq!(code_assist["tool_calls"][0]["signature"], "s");
        assert_eq!(code_assist["tool_calls"][1]["signature"], GEMINI_NO_SIGNATURE);
        let key = wire(&m, Target { signatures: Some(Wire::OpenAi), ..Target::default() });
        assert_eq!(key["tool_calls"][0]["extra_content"]["google"]["thought_signature"], "s");
        let other = wire(&m, Target::default()).to_string();
        assert!(!other.contains("signature") && !other.contains("extra_content"));
    }

    #[test]
    fn a_conversation_saved_with_gemini_signatures_still_loads() {
        let call: crate::domain::chat::ToolCall =
            serde_json::from_str(r#"{"id":"c1","name":"grep","arguments":"{}","signature":"sig"}"#).expect("champ retiré toléré");
        assert_eq!(call.name, "grep");
    }

    fn env_for(engine: Engine, model: &str) -> Env {
        use crate::domain::{auth::AuthMode, settings::Settings};
        let settings = Settings {
            engine,
            claude_auth: AuthMode::ApiKey,
            claude_key: "k".into(),
            deepseek_key: "k".into(),
            claude_model: model.into(),
            ..Settings::default()
        };
        let web = std::sync::Arc::new(crate::tools::webindex::WebIndex::open(&std::env::temp_dir().join("omega-ctx-web")));
        Env::new(settings, web).unwrap()
    }

    fn turn(added: &[ToolKind]) -> Conversation {
        let mut conv = Conversation::default();
        conv.toolset.extend(ToolKind::READ);
        conv.toolset.extend(added);
        conv.push(Message { tools_added: added.to_vec(), ..Message::user("cherche".into()) });
        conv
    }

    #[test]
    fn deferred_mode_keeps_the_tools_array_constant() {
        let env = env_for(Engine::Claude, "claude-opus-5-5");
        let (a, b) = (request(&turn(&[]), &env, "SYS", true), request(&turn(&[ToolKind::WebSearch]), &env, "SYS", true));
        assert_eq!(a["tools"], b["tools"]);
        let tools = a["tools"].as_array().unwrap();
        assert_eq!(tools.len(), ToolKind::ALL.len());
        for t in tools {
            let read = ToolKind::READ.iter().any(|k| t["name"] == k.name());
            assert_eq!(t.get("defer_loading").is_some(), !read, "{}", t["name"]);
        }
    }

    #[test]
    fn a_turn_that_adds_tools_appends_a_tool_addition_message() {
        let env = env_for(Engine::Claude, "claude-opus-5-5");
        let body = request(&turn(&[ToolKind::WebSearch]), &env, "SYS", true);
        let system = &body["messages"][1];
        assert_eq!(system["role"], "system");
        assert_eq!(system["content"][0]["type"], "tool_addition");
        assert_eq!(system["content"][0]["tool"], json!({ "type": "tool_reference", "name": "web_search" }));
    }

    #[test]
    fn an_interrupted_turn_hands_its_additions_to_the_next_one() {
        let env = env_for(Engine::Claude, "claude-opus-5-5");
        let mut conv = turn(&[ToolKind::WebSearch]);
        conv.push(Message { tools_added: vec![ToolKind::Shell], ..Message::user("encore".into()) });
        let body = request(&conv, &env, "SYS", true);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2, "user fusionné puis un seul message système");
        let names: Vec<&str> = messages[1]["content"].as_array().unwrap().iter().filter_map(|b| b["tool"]["name"].as_str()).collect();
        assert_eq!(names, ["web_search", "shell"]);
    }

    #[test]
    fn openai_wire_and_older_claude_ignore_tool_additions() {
        for env in [env_for(Engine::DeepSeek, "claude-opus-5-5"), env_for(Engine::Claude, "claude-sonnet-4-5")] {
            let body = request(&turn(&[ToolKind::WebSearch]), &env, "SYS", true);
            let text = body.to_string();
            assert!(!text.contains("tool_addition") && !text.contains("defer_loading"), "{text:.300}");
        }
    }

    #[test]
    fn a_rejected_beta_falls_back_to_classic_tools() {
        let env = env_for(Engine::Claude, "claude-opus-5-5");
        assert!(crate::infra::llm::tools_beta_rejected(&anyhow::anyhow!("Claude 400 Bad Request: tool_reference_unresolved")));
        assert!(!crate::infra::llm::tools_beta_rejected(&anyhow::anyhow!("Claude 401 Unauthorized: invalid x-api-key")));
        env.llm.disable_deferred();
        let body = request(&turn(&[ToolKind::WebSearch]), &env, "SYS", true);
        assert!(!body.to_string().contains("defer_loading"));
        assert_eq!(body["tools"].as_array().unwrap().len(), ToolKind::READ.len() + 1);
    }

    #[test]
    fn additions_are_what_the_visible_history_has_not_offered_yet() {
        let mut conv = turn(&[ToolKind::WebSearch]);
        conv.push(Message::assistant("ok".into(), String::new(), Vec::new()));
        conv.toolset.extend([ToolKind::WebSearch, ToolKind::Shell]);
        conv.push(Message::user("et maintenant".into()));
        assert_eq!(tool_additions(&conv, 0), [ToolKind::Shell]);
        assert_eq!(tool_additions(&conv, 2), [ToolKind::Shell, ToolKind::WebSearch], "résumé : ré-offrir");
    }
}
