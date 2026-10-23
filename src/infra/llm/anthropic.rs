//! Anthropic Messages wire (Claude API key and subscription tokens).

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Completion, Delta};
use crate::{
    domain::{chat::ToolCall, engine::Effort},
    infra::auth::claude::SYSTEM_PREFIX,
};

/// Ceiling per request; a safety cap, not a target.
const MAX_TOKENS_CAP: u32 = 32_000;
const NO_THINKING_MAX: u32 = 8_192;

/// Converts the canonical (OpenAI-shaped) history into a Messages request.
pub fn body(messages: &[Value], tools: Option<Value>, model: &str, effort: Effort, oauth: bool) -> Value {
    let system = system_blocks(messages, oauth);
    let known = tool_ids(messages);
    let mut out: Vec<Value> = Vec::new();
    let mut results: Vec<Value> = Vec::new();

    for m in messages {
        match m["role"].as_str().unwrap_or_default() {
            "system" => {}
            "assistant" => {
                flush(&mut out, &mut results);
                let mut blocks: Vec<Value> = Vec::new();
                let text = m["content"].as_str().unwrap_or_default();
                if !text.is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                for call in m["tool_calls"].as_array().into_iter().flatten() {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call["id"].as_str().unwrap_or_default(),
                        "name": call["function"]["name"].as_str().unwrap_or_default(),
                        "input": arguments(&call["function"]["arguments"]),
                    }));
                }
                if !blocks.is_empty() {
                    out.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
            "tool" => {
                let id = m["tool_call_id"].as_str().unwrap_or_default();
                let content = m["content"].as_str().unwrap_or_default();
                if known.iter().any(|k| k == id) {
                    results.push(json!({ "type": "tool_result", "tool_use_id": id, "content": content }));
                } else {
                    // Interrupted turn: the matching tool_use is gone, keep the text so the model sees it.
                    flush(&mut out, &mut results);
                    out.push(json!({ "role": "user", "content": [{ "type": "text", "text": content }] }));
                }
            }
            _ => {
                flush(&mut out, &mut results);
                out.push(json!({ "role": "user", "content": user_blocks(&m["content"]) }));
            }
        }
    }
    flush(&mut out, &mut results);

    let budget = effort.claude_budget();
    let mut body = json!({
        "model": model,
        "max_tokens": budget.map_or(NO_THINKING_MAX, |b| (b + NO_THINKING_MAX).min(MAX_TOKENS_CAP)),
        "messages": out,
        "stream": true,
    });
    if !system.is_empty() {
        body["system"] = system.into();
    }
    if oauth && let Some(budget) = budget {
        body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
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

fn tool_ids(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| m["tool_calls"].as_array())
        .flatten()
        .filter_map(|c| c["id"].as_str().map(str::to_owned))
        .collect()
}

fn flush(out: &mut Vec<Value>, results: &mut Vec<Value>) {
    if !results.is_empty() {
        out.push(json!({ "role": "user", "content": std::mem::take(results) }));
    }
}

/// Canonical (OpenAI-shaped) user content → Anthropic blocks; `data:` URLs become base64 images.
fn user_blocks(content: &Value) -> Vec<Value> {
    let Some(parts) = content.as_array() else {
        return vec![json!({ "type": "text", "text": content.as_str().unwrap_or_default() })];
    };
    parts
        .iter()
        .filter_map(|p| match p["type"].as_str().unwrap_or_default() {
            "text" => Some(json!({ "type": "text", "text": p["text"].as_str().unwrap_or_default() })),
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

pub fn apply(out: &mut Completion, data: &str, on: &mut impl FnMut(Delta<'_>)) -> Result<()> {
    let event: Event = serde_json::from_str(data).with_context(|| format!("flux invalide: {data:.160}"))?;
    match event.kind.as_str() {
        "message_start" => {
            let u = &event.message.as_ref().map_or(Value::Null, |m| m["usage"].clone());
            out.usage.prompt = u["input_tokens"].as_u64().unwrap_or(0);
            out.usage.cached = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
        }
        "content_block_start" => {
            let block = event.content_block.unwrap_or(Value::Null);
            if block["type"] == "tool_use" {
                out.blocks.insert(event.index, out.tool_calls.len());
                out.tool_calls.push(ToolCall {
                    id: block["id"].as_str().unwrap_or_default().to_owned(),
                    name: block["name"].as_str().unwrap_or_default().to_owned(),
                    arguments: String::new(),
                });
            }
        }
        "content_block_delta" => {
            let delta = event.delta.unwrap_or(Value::Null);
            match delta["type"].as_str().unwrap_or_default() {
                "thinking_delta" => {
                    let t = delta["thinking"].as_str().unwrap_or_default();
                    if !t.is_empty() {
                        on(Delta::Reasoning(t));
                        out.reasoning.push_str(t);
                    }
                }
                "text_delta" => {
                    let t = delta["text"].as_str().unwrap_or_default();
                    if !t.is_empty() {
                        on(Delta::Text(t));
                        out.content.push_str(t);
                    }
                }
                "input_json_delta" => {
                    if let Some(pos) = out.blocks.get(&event.index).copied()
                        && let Some(call) = out.tool_calls.get_mut(pos)
                    {
                        call.arguments.push_str(delta["partial_json"].as_str().unwrap_or_default());
                    }
                }
                _ => {}
            }
        }
        "message_delta" => {
            if let Some(u) = event.usage {
                out.usage.completion = u["output_tokens"].as_u64().unwrap_or(0);
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
        assert_eq!(b["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(b["messages"][1]["content"][0]["input"]["path"], "a.rs");
        assert_eq!(b["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(b["thinking"]["budget_tokens"], 4096);
        assert_eq!(b["stream"], true);
    }

    #[test]
    fn empty_tool_arguments_become_an_empty_object() {
        let msgs = vec![json!({ "role": "assistant", "content": "", "tool_calls": [
            { "id": "t1", "type": "function", "function": { "name": "shell", "arguments": "" } } ]})];
        let b = body(&msgs, None, "m", Effort::Off, false);
        assert_eq!(b["messages"][0]["content"][0]["input"], json!({}));
        assert!(b.get("thinking").is_none());
        assert!(b.get("system").is_none(), "aucun bloc système: le champ est omis");
        assert_eq!(b["max_tokens"], 8_192);
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

    #[test]
    fn stream_events_accumulate_text_thinking_and_input_json() {
        let mut out = Completion::default();
        for e in [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":10}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"why"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"grep"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"pat"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"tern\":\"x\"}"}}"#,
            r#"{"type":"message_delta","usage":{"output_tokens":5}}"#,
        ] {
            apply(&mut out, e, &mut |_| {}).unwrap();
        }
        assert_eq!((out.content.as_str(), out.reasoning.as_str()), ("ok", "why"));
        assert_eq!(out.tool_calls[0].arguments, r#"{"pattern":"x"}"#);
        assert_eq!((out.usage.prompt, out.usage.completion), (10, 5));
    }

    #[test]
    fn api_error_reads_the_message_field() {
        assert_eq!(error_text(r#"{"error":{"message":"bad key"}}"#), "bad key");
        assert_eq!(error_text("<html>oops</html>"), "<html>oops</html>");
    }
}
