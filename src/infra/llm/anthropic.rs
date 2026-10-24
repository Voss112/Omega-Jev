//! Anthropic Messages wire (Claude API key and subscription tokens).

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Completion, Delta};
use crate::{
    domain::{
        chat::{Part, ToolCall},
        engine::Effort,
    },
    infra::auth::claude::SYSTEM_PREFIX,
};

/// Output ceiling for the budget-thinking generation (Opus 4 / 4.1 stop at 32K).
const BUDGET_MAX_TOKENS: u32 = 32_000;
/// Adaptive models accept 128K; their thinking counts toward this ceiling.
const ADAPTIVE_MAX_TOKENS: u32 = 64_000;

/// How a model takes its reasoning settings.
#[derive(Debug, PartialEq, Eq)]
enum Thinking {
    /// Before the 4.6 generation: `{type: "enabled", budget_tokens}` (a 400 on newer models).
    Budget,
    /// 4.6 and later: `{type: "adaptive"}` + `output_config.effort`; `display` exists from 4.7 on,
    /// where the reasoning text is omitted unless asked for. From the 5 generation on, thinking cannot be turned off.
    Adaptive { display: bool, always_on: bool },
}

/// Generation of a model id: `claude-opus-4-1-20250805`, `claude-3-7-sonnet-latest`, `claude-opus-5-5`…
/// Unknown ids are assumed recent.
fn thinking(model: &str) -> Thinking {
    let mut family = None;
    let mut version: Vec<u32> = Vec::new();
    for token in model.split(['-', '.', '@']) {
        match token {
            "opus" | "sonnet" | "haiku" | "fable" | "mythos" => family = Some(token),
            t if !t.is_empty() && t.len() <= 2 && t.bytes().all(|b| b.is_ascii_digit()) => version.push(t.parse().unwrap_or(0)),
            _ => {}
        }
    }
    let (major, minor) = (version.first().copied().unwrap_or(0), version.get(1).copied().unwrap_or(0));
    if matches!(family, Some("opus" | "sonnet" | "haiku")) && major > 0 && (major, minor) < (4, 6) {
        Thinking::Budget
    } else {
        let always_on = matches!(family, Some("fable" | "mythos")) || major == 0 || major >= 5;
        Thinking::Adaptive { display: (major, minor) != (4, 6), always_on }
    }
}

/// Converts the canonical (OpenAI-shaped) history into a Messages request.
pub fn body(messages: &[Value], tools: Option<Value>, model: &str, effort: Effort, oauth: bool) -> Value {
    let mut system = system_blocks(messages, oauth);
    // Signed reasoning is replayed wherever the caller kept `parts` (append-only history: previous turns too,
    // so the cached prefix stays byte-identical). Older answers sent without `parts` form a leading run.
    let mut out: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // Ids of the latest assistant message: tool results pair with it (ids may repeat across steps).
    let mut pending: HashMap<String, String> = HashMap::new();

    for (i, m) in messages.iter().enumerate() {
        match m["role"].as_str().unwrap_or_default() {
            "system" => {}
            "assistant" => {
                let calls: Vec<&Value> = m["tool_calls"].as_array().into_iter().flatten().collect();
                let ids: Vec<String> = calls
                    .iter()
                    .enumerate()
                    .map(|(k, c)| {
                        let id = tool_id(c["id"].as_str().unwrap_or_default(), i, k, &seen);
                        seen.insert(id.clone());
                        id
                    })
                    .collect();
                pending =
                    calls.iter().zip(&ids).map(|(c, id)| (c["id"].as_str().unwrap_or_default().to_owned(), id.clone())).collect();
                let blocks = assistant_blocks(m, &calls, &ids);
                if !blocks.is_empty() {
                    push(&mut out, "assistant", blocks);
                }
            }
            "tool" => {
                let content = m["content"].as_str().filter(|c| !c.trim().is_empty()).unwrap_or("(no output)");
                let block = match pending.remove(m["tool_call_id"].as_str().unwrap_or_default()) {
                    Some(id) => json!({ "type": "tool_result", "tool_use_id": id, "content": content }),
                    // Interrupted turn: the matching tool_use is gone, keep the text so the model sees it.
                    None => json!({ "type": "text", "text": content }),
                };
                push(&mut out, "user", vec![block]);
            }
            _ => {
                pending.clear();
                let blocks = user_blocks(&m["content"]);
                if !blocks.is_empty() {
                    push(&mut out, "user", blocks);
                }
            }
        }
    }

    // Two cache breakpoints: the static system prompt, and the end of the history (read back next step).
    let ephemeral = json!({ "type": "ephemeral" });
    if let Some(last) = system.last_mut() {
        last["cache_control"] = ephemeral.clone();
    }
    if let Some(block) = out.last_mut().and_then(|m| m["content"].as_array_mut()).and_then(|c| c.last_mut())
        && !matches!(block["type"].as_str(), Some("thinking" | "redacted_thinking"))
    {
        block["cache_control"] = ephemeral;
    }

    let mut body = json!({ "model": model, "messages": out, "stream": true });
    if !system.is_empty() {
        body["system"] = system.into();
    }
    match thinking(model) {
        Thinking::Budget => {
            body["max_tokens"] = json!(BUDGET_MAX_TOKENS);
            if let Some(budget) = effort.claude_budget() {
                body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
            }
        }
        Thinking::Adaptive { display, always_on } => {
            body["max_tokens"] = json!(ADAPTIVE_MAX_TOKENS);
            if effort != Effort::Off {
                body["output_config"] = json!({ "effort": effort.claude_effort() });
                // The UI shows the reasoning: ask for its summary instead of the default empty text.
                body["thinking"] =
                    if display { json!({ "type": "adaptive", "display": "summarized" }) } else { json!({ "type": "adaptive" }) };
            } else if always_on {
                // Off cannot disable thinking here (a 400): the lowest effort keeps it shortest.
                body["output_config"] = json!({ "effort": effort.claude_effort() });
            }
        }
    }
    if let Some(tools) = tools {
        body["tools"] = tools.as_array().into_iter().flatten().map(tool_schema).collect::<Vec<Value>>().into();
    }
    body
}

fn system_blocks(messages: &[Value], oauth: bool) -> Vec<Value> {
    let mut blocks = Vec::new();
    if oauth {
        blocks.push(json!({ "type": "text", "text": SYSTEM_PREFIX }));
    }
    blocks.extend(messages.iter().filter(|m| m["role"] == "system").filter_map(|m| {
        let text = m["content"].as_str().filter(|t| !t.is_empty())?;
        Some(json!({ "type": "text", "text": text }))
    }));
    blocks
}

/// Anthropic ids are `[A-Za-z0-9_-]+` and unique; foreign ids (`functions.grep:0` from stored conversations, repeated across steps)
/// get a clean per-message form, identical on every request.
fn tool_id(raw: &str, message: usize, call: usize, seen: &HashSet<String>) -> String {
    let valid = !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if valid && !seen.contains(raw) {
        return raw.to_owned();
    }
    let clean: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    format!("{clean}_{message}_{call}")
}

/// The answer's blocks: its recorded layout (signed reasoning included) when Claude produced it,
/// otherwise its text then its calls.
fn assistant_blocks(m: &Value, calls: &[&Value], ids: &[String]) -> Vec<Value> {
    let tool_use = |k: usize| {
        json!({
            "type": "tool_use",
            "id": ids[k],
            "name": calls[k]["function"]["name"].as_str().unwrap_or_default(),
            "input": arguments(&calls[k]["function"]["arguments"]),
        })
    };
    let text = |t: &str| (!t.trim().is_empty()).then(|| json!({ "type": "text", "text": t }));
    let Some(parts) = m["parts"].as_array().filter(|p| !p.is_empty()) else {
        let mut blocks: Vec<Value> = text(m["content"].as_str().unwrap_or_default()).into_iter().collect();
        blocks.extend((0..calls.len()).map(tool_use));
        return blocks;
    };
    let mut used = vec![false; calls.len()];
    let mut blocks = Vec::new();
    for p in parts {
        match p["type"].as_str().unwrap_or_default() {
            "thinking" => {
                blocks.push(json!({ "type": "thinking", "thinking": p["thinking"], "signature": p["signature"] }));
            }
            "redacted_thinking" => blocks.push(json!({ "type": "redacted_thinking", "data": p["data"] })),
            "text" => blocks.extend(text(p["text"].as_str().unwrap_or_default())),
            "tool_use" => {
                let k = p["call"].as_u64().and_then(|k| usize::try_from(k).ok()).filter(|k| *k < calls.len() && !used[*k]);
                if let Some(k) = k {
                    used[k] = true;
                    blocks.push(tool_use(k));
                }
            }
            _ => {}
        }
    }
    blocks.extend((0..calls.len()).filter(|k| !used[*k]).map(tool_use));
    blocks
}

/// Appends to the previous message when it has the same role (tool results, then the next prompt);
/// tool results stay ahead of any text, as the API requires.
fn push(out: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if let Some(last) = out.last_mut().filter(|m| m["role"] == role)
        && let Some(content) = last["content"].as_array_mut()
    {
        content.extend(blocks);
        content.sort_by_key(|b| b["type"] != "tool_result");
        return;
    }
    out.push(json!({ "role": role, "content": blocks }));
}

/// Canonical (OpenAI-shaped) user content → Anthropic blocks; `data:` URLs become base64 images.
fn user_blocks(content: &Value) -> Vec<Value> {
    let Some(parts) = content.as_array() else {
        let text = content.as_str().unwrap_or_default();
        return if text.trim().is_empty() { Vec::new() } else { vec![json!({ "type": "text", "text": text })] };
    };
    parts
        .iter()
        .filter_map(|p| match p["type"].as_str().unwrap_or_default() {
            "text" => {
                let text = p["text"].as_str().filter(|t| !t.trim().is_empty())?;
                Some(json!({ "type": "text", "text": text }))
            }
            "image_url" => {
                let (media, data) = data_url(p["image_url"]["url"].as_str()?)?;
                Some(json!({ "type": "image", "source": { "type": "base64", "media_type": media, "data": data } }))
            }
            _ => None,
        })
        .collect()
}

fn data_url(url: &str) -> Option<(&str, &str)> {
    url.strip_prefix("data:")?.split_once(";base64,")
}

/// Empty or malformed arguments become `{}` so the tool can report a missing argument instead of crashing.
fn arguments(raw: &Value) -> Value {
    raw.as_str()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn tool_schema(schema: &Value) -> Value {
    json!({
        "name": schema["function"]["name"],
        "description": schema["function"]["description"],
        "input_schema": schema["function"]["parameters"],
    })
}

/// Prompt size counts cached reads and writes: `input_tokens` alone is only the uncached tail.
fn input_usage(out: &mut Completion, u: &Value) {
    let n = |key: &str| u[key].as_u64().unwrap_or(0);
    let prompt = n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
    if prompt > 0 {
        out.usage.prompt = prompt;
        out.usage.cached = n("cache_read_input_tokens");
        out.usage.written = n("cache_creation_input_tokens");
    }
}

/// Display only: consecutive blocks read as separate paragraphs.
fn separate(acc: &mut String, on: &mut impl FnMut(Delta<'_>), reasoning: bool) {
    if !acc.is_empty() && !acc.ends_with("\n\n") {
        acc.push_str("\n\n");
        on(if reasoning { Delta::Reasoning("\n\n") } else { Delta::Text("\n\n") });
    }
}

pub fn apply(out: &mut Completion, data: &str, on: &mut impl FnMut(Delta<'_>)) -> Result<()> {
    let event: Event = serde_json::from_str(data).with_context(|| format!("flux invalide: {data:.160}"))?;
    match event.kind.as_str() {
        "message_start" => {
            let usage = event.message.as_ref().map_or(Value::Null, |m| m["usage"].clone());
            input_usage(out, &usage);
        }
        "content_block_start" => {
            let block = event.content_block.unwrap_or(Value::Null);
            let initial = |key: &str| block[key].as_str().unwrap_or_default().to_owned();
            let part = match block["type"].as_str().unwrap_or_default() {
                "thinking" => {
                    separate(&mut out.reasoning, on, true);
                    Part::Thinking { thinking: initial("thinking"), signature: initial("signature") }
                }
                "redacted_thinking" => Part::RedactedThinking { data: initial("data") },
                "text" => {
                    separate(&mut out.content, on, false);
                    Part::Text { text: String::new() }
                }
                "tool_use" => {
                    out.tool_calls.push(ToolCall { id: initial("id"), name: initial("name"), ..ToolCall::default() });
                    Part::ToolUse { call: out.tool_calls.len() - 1 }
                }
                // Server-side tool blocks and future types: nothing this client can replay.
                _ => return Ok(()),
            };
            out.blocks.insert(event.index, out.parts.len());
            out.parts.push(part);
        }
        "content_block_delta" => {
            let delta = event.delta.unwrap_or(Value::Null);
            let piece = |key: &str| delta[key].as_str().unwrap_or_default();
            let part = out.blocks.get(&event.index).copied().and_then(|p| out.parts.get_mut(p));
            match (delta["type"].as_str().unwrap_or_default(), part) {
                ("thinking_delta", part) => {
                    let t = piece("thinking");
                    if let Some(Part::Thinking { thinking, .. }) = part {
                        thinking.push_str(t);
                    }
                    if !t.is_empty() {
                        on(Delta::Reasoning(t));
                        out.reasoning.push_str(t);
                    }
                }
                ("signature_delta", Some(Part::Thinking { signature, .. })) => signature.push_str(piece("signature")),
                ("text_delta", part) => {
                    let t = piece("text");
                    if let Some(Part::Text { text }) = part {
                        text.push_str(t);
                    }
                    if !t.is_empty() {
                        on(Delta::Text(t));
                        out.content.push_str(t);
                    }
                }
                ("input_json_delta", Some(Part::ToolUse { call })) => {
                    let call = *call;
                    if let Some(c) = out.tool_calls.get_mut(call) {
                        c.arguments.push_str(piece("partial_json"));
                    }
                }
                _ => {}
            }
        }
        "message_delta" => {
            if let Some(d) = &event.delta {
                if let Some(reason) = d["stop_reason"].as_str() {
                    reason.clone_into(&mut out.stop);
                }
                let details = &d["stop_details"];
                if let Some(detail) = details["category"].as_str().or_else(|| details["explanation"].as_str()) {
                    detail.clone_into(&mut out.stop_detail);
                }
            }
            if let Some(u) = event.usage {
                out.usage.completion = u["output_tokens"].as_u64().unwrap_or(out.usage.completion);
                input_usage(out, &u);
            }
        }
        "error" => {
            let detail = event.error.as_ref().map_or(Value::Null, Clone::clone);
            anyhow::bail!("Claude : {}", detail["message"].as_str().unwrap_or("erreur de flux"));
        }
        _ => {}
    }
    Ok(())
}

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    index: usize,
    message: Option<Value>,
    content_block: Option<Value>,
    delta: Option<Value>,
    usage: Option<Value>,
    error: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::llm::error_text;

    #[test]
    fn messages_are_converted_with_tools() {
        let msgs = vec![
            json!({ "role": "system", "content": "SYS" }),
            json!({ "role": "user", "content": "hi" }),
            json!({ "role": "assistant", "content": "", "tool_calls": [
                { "id": "t1", "type": "function", "function": { "name": "read_file", "arguments": "{\"path\":\"a.rs\"}" } }
            ]}),
            json!({ "role": "tool", "tool_call_id": "t1", "content": "1  code" }),
        ];
        let b = body(&msgs, None, "claude-sonnet-4-5", Effort::Medium, true);
        assert_eq!(b["system"][0]["text"], SYSTEM_PREFIX);
        assert_eq!(b["system"][1]["text"], "SYS");
        assert_eq!(b["system"][1]["cache_control"]["type"], "ephemeral");
        assert_eq!(b["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(b["messages"][1]["content"][0]["input"]["path"], "a.rs");
        assert_eq!(b["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(b["messages"][2]["content"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(b["thinking"]["budget_tokens"], 4096);
        assert!(b.get("output_config").is_none(), "pas d'effort sur la génération à budget");
        assert_eq!(b["stream"], true);
    }

    #[test]
    fn every_generation_gets_the_thinking_shape_it_accepts() {
        assert_eq!(thinking("claude-sonnet-4-5"), Thinking::Budget);
        assert_eq!(thinking("claude-opus-4-1-20250805"), Thinking::Budget);
        assert_eq!(thinking("claude-3-7-sonnet-latest"), Thinking::Budget);
        assert_eq!(thinking("claude-sonnet-4-6"), Thinking::Adaptive { display: false, always_on: false });
        assert_eq!(thinking("claude-opus-4-7"), Thinking::Adaptive { display: true, always_on: false });
        assert_eq!(thinking("claude-opus-5-5"), Thinking::Adaptive { display: true, always_on: true });
        assert_eq!(thinking("claude-fable-5-1"), Thinking::Adaptive { display: true, always_on: true });
        let quiet = body(&[json!({ "role": "user", "content": "x" })], None, "claude-opus-4-8", Effort::Off, false);
        assert!(quiet.get("thinking").is_none() && quiet.get("output_config").is_none(), "Off : aucun paramètre en 4.x");
        let b = body(&[json!({ "role": "user", "content": "x" })], None, "claude-opus-5-5", Effort::High, true);
        assert_eq!((b["thinking"]["type"].as_str(), b["thinking"]["display"].as_str()), (Some("adaptive"), Some("summarized")));
        assert!(b["thinking"].get("budget_tokens").is_none(), "budget_tokens est refusé (400) sur Claude 5");
        assert_eq!((b["output_config"]["effort"].as_str(), b["max_tokens"].as_u64()), (Some("high"), Some(64_000)));
        let off = body(&[json!({ "role": "user", "content": "x" })], None, "claude-opus-5-5", Effort::Off, false);
        assert!(off.get("thinking").is_none(), "Off : pas de paramètre thinking (le désactiver est une 400)");
        assert_eq!(off["output_config"]["effort"], "low");
    }

    #[test]
    fn empty_tool_arguments_become_an_empty_object() {
        let msgs = vec![
            json!({ "role": "user", "content": "go" }),
            json!({ "role": "assistant", "content": "", "tool_calls": [
            { "id": "t1", "type": "function", "function": { "name": "shell", "arguments": "" } } ]}),
        ];
        let b = body(&msgs, None, "m", Effort::Off, false);
        assert_eq!(b["messages"][1]["content"][0]["input"], json!({}));
        assert!(b.get("thinking").is_none());
        assert!(b.get("system").is_none(), "aucun bloc système: le champ est omis");
    }

    #[test]
    fn orphan_tool_results_become_user_text() {
        let msgs = vec![json!({ "role": "tool", "tool_call_id": "gone", "content": "ERROR" })];
        let b = body(&msgs, None, "m", Effort::Off, false);
        assert_eq!(b["messages"][0]["role"], "user");
        assert_eq!(b["messages"][0]["content"][0]["text"], "ERROR");
    }

    #[test]
    fn a_data_url_becomes_a_base64_image_block() {
        let msgs = vec![json!({ "role": "user", "content": [
            { "type": "text", "text": "regarde" },
            { "type": "image_url", "image_url": { "url": "data:image/png;base64,QUJD" } },
        ]})];
        let b = body(&msgs, None, "m", Effort::Off, false);
        assert_eq!(b["messages"][0]["content"][0]["text"], "regarde");
        assert_eq!(b["messages"][0]["content"][1]["type"], "image");
        assert_eq!(b["messages"][0]["content"][1]["source"]["media_type"], "image/png");
        assert_eq!(b["messages"][0]["content"][1]["source"]["data"], "QUJD");
    }

    fn signed_answer(id: &str) -> Value {
        json!({ "role": "assistant", "content": "je regarde", "tool_calls": [
            { "id": id, "type": "function", "function": { "name": "grep", "arguments": "{}" } } ],
            "parts": [
                { "type": "thinking", "thinking": "", "signature": "sig" },
                { "type": "text", "text": "je regarde" },
                { "type": "tool_use", "call": 0 },
            ] })
    }

    #[test]
    fn signed_reasoning_is_replayed_in_order_on_every_turn_that_kept_it() {
        let mut stripped = signed_answer("toolu_a");
        stripped.as_object_mut().unwrap().remove("parts");
        let msgs = vec![
            json!({ "role": "user", "content": "un" }),
            stripped,
            json!({ "role": "tool", "tool_call_id": "toolu_a", "content": "1 hit" }),
            json!({ "role": "assistant", "content": "fini" }),
            json!({ "role": "user", "content": "deux" }),
            signed_answer("toolu_b"),
            json!({ "role": "tool", "tool_call_id": "toolu_b", "content": "2 hits" }),
            json!({ "role": "assistant", "content": "ok" }),
            json!({ "role": "user", "content": "trois" }),
        ];
        let b = body(&msgs, None, "claude-opus-5-5", Effort::Medium, true);
        let old = &b["messages"][1]["content"];
        assert_eq!(
            (old[0]["type"].as_str(), old[1]["type"].as_str()),
            (Some("text"), Some("tool_use")),
            "série de tête sans parts : sans pensée"
        );
        let previous = &b["messages"][5]["content"];
        assert_eq!(previous[0], json!({ "type": "thinking", "thinking": "", "signature": "sig" }), "tour clos rejoué tel quel");
        assert_eq!((previous[1]["type"].as_str(), previous[2]["type"].as_str()), (Some("text"), Some("tool_use")));
    }

    #[test]
    fn foreign_tool_ids_are_made_valid_unique_and_paired() {
        let call = json!({ "id": "functions.grep:0", "type": "function", "function": { "name": "grep", "arguments": "{}" } });
        let msgs = vec![
            json!({ "role": "user", "content": "x" }),
            json!({ "role": "assistant", "content": "", "tool_calls": [call.clone()] }),
            json!({ "role": "tool", "tool_call_id": "functions.grep:0", "content": "a" }),
            json!({ "role": "assistant", "content": "", "tool_calls": [call] }),
            json!({ "role": "tool", "tool_call_id": "functions.grep:0", "content": "b" }),
            json!({ "role": "user", "content": "et ensuite ?" }),
        ];
        let b = body(&msgs, None, "m", Effort::Off, false);
        let (first, second) = (&b["messages"][1]["content"][0]["id"], &b["messages"][3]["content"][0]["id"]);
        assert_ne!(first, second, "ids uniques");
        assert!(first.as_str().unwrap().bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'));
        assert_eq!(&b["messages"][2]["content"][0]["tool_use_id"], first);
        let last = &b["messages"][4]["content"];
        assert_eq!(&last[0]["tool_use_id"], second);
        assert_eq!(last[1]["text"], "et ensuite ?", "le prompt suivant rejoint les résultats, après eux");
        assert_eq!(b["messages"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn stream_events_accumulate_text_thinking_and_input_json() {
        let mut out = Completion::default();
        for e in [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":90}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"why"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"grep"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"pat"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"tern\":\"x\"}"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}"#,
        ] {
            apply(&mut out, e, &mut |_| {}).unwrap();
        }
        assert_eq!((out.content.as_str(), out.reasoning.as_str()), ("ok", "why"));
        assert_eq!(out.tool_calls[0].arguments, r#"{"pattern":"x"}"#);
        assert_eq!((out.usage.prompt, out.usage.cached, out.usage.completion), (100, 90, 5), "lectures de cache comprises");
        assert_eq!(out.stop, "tool_use");
    }

    #[test]
    fn thinking_signatures_and_block_order_are_captured() {
        let mut out = Completion::default();
        for e in [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"EqQB"}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"opaque"}}"#,
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Voici"}}"#,
            r#"{"type":"content_block_start","index":3,"content_block":{"type":"tool_use","id":"toolu_1","name":"grep"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"category":"cyber"}}}"#,
        ] {
            apply(&mut out, e, &mut |_| {}).unwrap();
        }
        assert_eq!(
            out.parts,
            vec![
                Part::Thinking { thinking: "plan".into(), signature: "EqQB".into() },
                Part::RedactedThinking { data: "opaque".into() },
                Part::Text { text: "Voici".into() },
                Part::ToolUse { call: 0 },
            ]
        );
        assert_eq!((out.stop.as_str(), out.stop_detail.as_str()), ("refusal", "cyber"));
    }

    #[test]
    fn api_error_reads_the_message_field() {
        assert_eq!(error_text(r#"{"error":{"message":"bad key"}}"#), "bad key");
        assert_eq!(error_text("<html>oops</html>"), "<html>oops</html>");
    }
}
