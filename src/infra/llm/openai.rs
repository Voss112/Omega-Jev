//! OpenAI-compatible wire (DeepSeek).

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Completion, Delta};
use crate::domain::{
    chat::{ToolCall, Usage},
    engine::{Effort, Engine},
};

pub const MAX_OUTPUT: u32 = 65_536;

/// Streaming request body with the user's effort.
pub fn body(model: &str, engine: Engine, effort: Effort, messages: &[Value], tools: Option<Value>) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if let Some(e) = effort.openai() {
        body["reasoning_effort"] = json!(e);
    }
    if engine == Engine::DeepSeek {
        body["max_tokens"] = json!(MAX_OUTPUT);
        body["thinking"] = json!({ "type": "enabled" });
    }
    if let Some(tools) = tools {
        body["tools"] = tools;
    }
    body
}

pub fn apply(out: &mut Completion, data: &str, on: &mut impl FnMut(Delta<'_>)) -> Result<()> {
    let raw: Value = serde_json::from_str(data).with_context(|| format!("flux invalide: {data:.160}"))?;
    if let Some(message) = super::stream_error(&raw) {
        anyhow::bail!("erreur du fournisseur pendant le flux : {message}");
    }
    let chunk: Chunk = serde_json::from_value(raw).with_context(|| format!("flux invalide: {data:.160}"))?;
    if let Some(u) = chunk.usage.as_ref().or_else(|| chunk.choices.iter().find_map(|c| c.usage.as_ref())) {
        out.usage = usage(u);
    }
    for choice in chunk.choices {
        if let Some(reason) = choice.finish_reason.filter(|r| !r.is_empty()) {
            out.stop = reason;
        }
        let delta = choice.delta;
        if let Some(t) = delta.reasoning_content.filter(|t| !t.is_empty()) {
            on(Delta::Reasoning(&t));
            out.reasoning.push_str(&t);
        }
        if let Some(t) = delta.content.filter(|t| !t.is_empty()) {
            on(Delta::Text(&t));
            out.content.push_str(&t);
        }
        for wire in delta.tool_calls {
            if out.tool_calls.len() <= wire.index {
                out.tool_calls.resize_with(wire.index + 1, ToolCall::default);
            }
            let call = &mut out.tool_calls[wire.index];
            if let Some(id) = wire.id {
                call.id = id;
            }
            if let Some(f) = wire.function {
                call.name.push_str(f.name.as_deref().unwrap_or_default());
                call.arguments.push_str(f.arguments.as_deref().unwrap_or_default());
            }
        }
    }
    Ok(())
}

/// Usage with every known cached-token field (OpenAI `prompt_tokens_details`, top-level `cached_tokens`, DeepSeek-style).
pub fn usage(u: &Value) -> Usage {
    let n = |ptr: &str| u.pointer(ptr).and_then(Value::as_u64);
    Usage {
        prompt: n("/prompt_tokens").unwrap_or(0),
        completion: n("/completion_tokens").unwrap_or(0),
        cached: n("/prompt_tokens_details/cached_tokens")
            .or_else(|| n("/cached_tokens"))
            .or_else(|| n("/prompt_cache_hit_tokens"))
            .unwrap_or(0),
        reasoning: n("/completion_tokens_details/reasoning_tokens").unwrap_or(0),
        jev: 0,
        written: 0,
    }
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<Choice>,
    usage: Option<Value>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: WireDelta,
    /// Some OpenAI-compatible servers put the final usage inside the choice.
    usage: Option<Value>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct WireDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireCall>,
}

#[derive(Deserialize)]
struct WireCall {
    index: usize,
    id: Option<String>,
    function: Option<WireFn>,
}

#[derive(Deserialize)]
struct WireFn {
    name: Option<String>,
    arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_tool_calls_and_usage_are_accumulated() {
        let mut out = Completion::default();
        let mut seen = String::new();
        let chunks = [
            r#"{"choices":[{"delta":{"reasoning_content":"think "}}]}"#,
            r#"{"choices":[{"delta":{"content":"Hi"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"grep","arguments":"{\"pat"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"tern\":\"x\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"usage":{"prompt_tokens":100,"completion_tokens":5,"cached_tokens":80}}]}"#,
        ];
        for c in chunks {
            apply(&mut out, c, &mut |d| {
                if let Delta::Text(t) = d {
                    seen.push_str(t);
                }
            })
            .unwrap();
        }
        assert_eq!((out.content.as_str(), out.reasoning.as_str(), seen.as_str()), ("Hi", "think ", "Hi"));
        assert_eq!(out.tool_calls[0].arguments, r#"{"pattern":"x"}"#);
        assert_eq!((out.usage.prompt, out.usage.cached), (100, 80));
    }

    #[test]
    fn an_error_inside_the_stream_is_reported() {
        let mut out = Completion::default();
        let err = apply(&mut out, r#"{"error":{"message":"quota exceeded"}}"#, &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("quota exceeded"));
        apply(&mut out, r#"{"choices":[{"delta":{},"finish_reason":"content_filter"}]}"#, &mut |_| {}).unwrap();
        assert_eq!(out.stop, "content_filter");
    }

    #[test]
    fn tools_pass_through_unchanged_and_off_omits_reasoning() {
        let tools = json!([
            { "type": "function", "function": { "name": "audit_kit", "parameters": { "type": "object", "properties": {} } } },
        ]);
        let b = body("deepseek-flash", Engine::DeepSeek, Effort::Max, &[], Some(tools.clone()));
        assert_eq!(b["reasoning_effort"], "max");
        assert_eq!(b["tools"], tools);
        assert!(body("deepseek-flash", Engine::DeepSeek, Effort::Off, &[], None).get("reasoning_effort").is_none());
    }
}
