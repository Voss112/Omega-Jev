//! Per-request economy measurement and the local proofs behind the « Économie tokens » health lines.

use std::{
    collections::BTreeSet,
    hash::{DefaultHasher, Hash, Hasher},
};

use anyhow::{Result, ensure};
use serde_json::Value;

use super::{Env, clear, context, router};
use crate::{
    domain::{
        auth::AuthMode,
        chat::{Conversation, Message, Part, Role, ToolCall, Usage},
        engine::Engine,
        playbook::Playbook,
        savings::{Levers, Rates, Sample},
        settings::Settings,
        tool::ToolKind,
    },
    infra::llm::Llm,
    tools::{schema, webindex::WebIndex},
};

/// What one request cost (provider usage) and what each lever kept off it (bytes measured on `body`).
pub fn measure(conv: &Conversation, env: &Env, body: &Value, usage: &Usage) -> Sample {
    let mut images = false;
    let bytes = text_bytes(body, &mut images);
    let from = conv.summary.as_ref().map_or(0, |s| s.upto).min(conv.messages.len());
    let visible = &conv.messages[from..];
    let levers = Levers {
        lazy: schema_bytes(&ToolKind::ALL.into_iter().collect()).saturating_sub(schema_bytes(&conv.toolset)),
        rtk: visible.iter().filter(|m| m.role == Role::Tool).map(|m| m.rtk).sum(),
        playbook: visible.iter().filter_map(skipped_playbook).sum(),
        summary: conv.summary.as_ref().map_or(0, |s| {
            conv.messages[..from].iter().map(message_bytes).sum::<u64>().saturating_sub(s.text.len() as u64)
        }),
        cleared: clear::saved_bytes(visible),
    };
    Sample {
        prompt: usage.prompt,
        cached: usage.cached,
        written: usage.written,
        completion: usage.completion,
        bytes,
        images,
        levers,
        rates: Rates::of(env.llm.engine, &env.llm.model),
    }
}

/// A request without any lever (compaction call), billed at the rates of the engine that served it.
pub fn plain(llm: &Llm, body: &Value, usage: &Usage) -> Sample {
    let mut images = false;
    let bytes = text_bytes(body, &mut images);
    Sample {
        prompt: usage.prompt,
        cached: usage.cached,
        written: usage.written,
        completion: usage.completion,
        bytes,
        images,
        levers: Levers::default(),
        rates: Rates::of(llm.engine, &llm.model),
    }
}

/// Fingerprint of what Claude's signed reasoning is bound to besides the history: system prompt and tool set.
pub fn prefix(system: &str, toolset: &BTreeSet<ToolKind>) -> u64 {
    let mut h = DefaultHasher::new();
    system.hash(&mut h);
    toolset.hash(&mut h);
    h.finish().max(1)
}

pub fn schema_bytes(set: &BTreeSet<ToolKind>) -> u64 {
    if set.is_empty() { 0 } else { schema::schemas(set).to_string().len() as u64 }
}

/// Text the provider tokenizes: images (priced by pixels) and signatures (not text) excluded.
fn text_bytes(v: &Value, images: &mut bool) -> u64 {
    match v {
        Value::String(s) if s.starts_with("data:") => {
            *images = true;
            0
        }
        Value::String(s) => s.len() as u64,
        Value::Array(items) => items.iter().map(|x| text_bytes(x, images)).sum(),
        Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("base64") => {
            *images = true;
            0
        }
        Value::Object(o) => o
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "signature" | "cache_control"))
            .map(|(k, x)| k.len() as u64 + text_bytes(x, images))
            .sum(),
        other => other.to_string().len() as u64,
    }
}

/// Bytes of a playbook body that this user turn referenced instead of re-sending.
fn skipped_playbook(m: &Message) -> Option<u64> {
    let id = m.route.as_ref()?.playbook.as_deref()?;
    let book = Playbook::find(id)?;
    m.preamble.contains(&format!("[playbook:{id}] Follow it again")).then(|| book.body().len() as u64)
}

fn message_bytes(m: &Message) -> u64 {
    let calls: usize = m.tool_calls.iter().map(|c| c.name.len() + c.arguments.len()).sum();
    (m.content.len() + m.preamble.len() + m.reasoning.len() + calls) as u64
}

/// The tool loop and the next turn only append: request N is a byte-identical prefix of request N+1, on both wires.
pub fn prefix_self_test(web: &std::sync::Arc<WebIndex>) -> Result<String> {
    let mut checked = Vec::new();
    for engine in [Engine::DeepSeek, Engine::Claude] {
        let settings = Settings {
            engine,
            claude_auth: AuthMode::ApiKey,
            claude_key: "k".into(),
            deepseek_key: "k".into(),
            claude_model: "claude-opus-5-5".into(),
            ..Settings::default()
        };
        let env = Env::new(settings, web.clone())?;
        let mut conv = Conversation::default();
        conv.toolset.extend(ToolKind::READ);
        conv.push(Message::user("cherche run".into()));
        let call = ToolCall { id: "toolu_1".into(), name: "grep".into(), arguments: r#"{"pattern":"run"}"#.into() };
        let parts = vec![Part::Thinking { thinking: String::new(), signature: "sig".into() }, Part::ToolUse { call: 0 }];
        conv.push(Message { parts, ..Message::assistant(String::new(), "r".into(), vec![call]) });
        conv.push(Message::tool("toolu_1".into(), "x".repeat(5000)));
        conv.push(Message::assistant("fini".into(), String::new(), Vec::new()));
        let mut earlier = context::request(&conv, &env, "SYS", true);
        for turn in 0..4 {
            conv.push(Message::user(format!("suite {turn}")));
            let later = context::request(&conv, &env, "SYS", true);
            ensure!(is_prefix(&earlier, &later), "{} : historique réécrit au tour {turn} (cache cassé)", engine.label());
            earlier = later;
        }
        checked.push(engine.label());
    }
    Ok(format!("historique append-only vérifié ({})", checked.join(", ")))
}

/// `a`'s messages (cache markers ignored) open `b`'s, with the same tools and system.
fn is_prefix(a: &Value, b: &Value) -> bool {
    let clean = |v: &Value| {
        let mut v = v.clone();
        strip_markers(&mut v);
        v
    };
    let (a, b) = (clean(a), clean(b));
    let (Some(ma), Some(mb)) = (a["messages"].as_array(), b["messages"].as_array()) else { return false };
    // The last message of `a` may grow (Anthropic merges the next user text into the tool-result message).
    let stable = ma.len().saturating_sub(1);
    a["tools"] == b["tools"] && a["system"] == b["system"] && mb.len() >= ma.len() && ma[..stable] == mb[..stable]
}

fn strip_markers(v: &mut Value) {
    match v {
        Value::Object(o) => {
            o.remove("cache_control");
            o.values_mut().for_each(strip_markers);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_markers),
        _ => {}
    }
}

/// Lazy tools: a chat turn ships no schema, a code turn only the code belt, an app turn adds `cli_anything`.
pub fn lazy_self_test() -> Result<String> {
    let all = schema_bytes(&ToolKind::ALL.into_iter().collect());
    let chat = schema_bytes(&router::local("merci, bonne journée").tools());
    let code = schema_bytes(&router::local("corrige le bug dans main.rs").tools());
    let apps = router::local("recadre photo.png avec gimp").tools();
    ensure!(chat < code && code < all, "ensembles d'outils non sélectifs ({chat} / {code} / {all} octets)");
    ensure!(apps.contains(&ToolKind::CliAnything), "une demande GIMP ne charge pas cli_anything");
    Ok(format!("schémas : chat {chat} o · code {code} o · tous {all} o (−{} % en code)", 100 - code * 100 / all))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_history_is_append_only_on_both_engines() {
        let web = std::sync::Arc::new(WebIndex::open(&std::env::temp_dir().join("omega-savings-web")));
        prefix_self_test(&web).unwrap();
        lazy_self_test().unwrap();
    }

    #[test]
    fn images_and_signatures_are_not_counted_as_text() {
        let mut images = false;
        let v = serde_json::json!({ "a": "1234", "signature": "x".repeat(99), "img": { "type": "base64", "data": "QUJD" } });
        assert_eq!(text_bytes(&v, &mut images), 1 + 4 + 3);
        assert!(images);
    }
}
