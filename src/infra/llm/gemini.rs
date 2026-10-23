//! Gemini Code Assist wire (`v1internal`, subscription tokens only).

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{Completion, Delta};
use crate::{
    domain::{chat::ToolCall, engine::Effort},
    infra::auth::gemini,
};

const MAX_OUTPUT: u32 = 65_536;

/// Converts the canonical history into a `v1internal` envelope.
pub fn body(messages: &[Value], tools: Option<Value>, model: &str, project: &str, effort: Effort) -> Value {
    let names = call_names(messages);
    let mut contents: Vec<Value> = Vec::new();
    for m in messages {
        match m["role"].as_str().unwrap_or_default() {
            "system" => {}
            "assistant" => {
                let mut parts: Vec<Value> = Vec::new();
                if let Some(text) = m["content"].as_str().filter(|t| !t.is_empty()) {
                    parts.push(json!({ "text": text }));
                }
                for call in m["tool_calls"].as_array().into_iter().flatten() {
                    parts.push(json!({
                        "functionCall": {
                            "name": call["function"]["name"],
                            "args": arguments(&call["function"]["arguments"]),
                        }
                    }));
                }
                if !parts.is_empty() {
                    contents.push(json!({ "role": "model", "parts": parts }));
                }
            }
            "tool" => {
                let id = m["tool_call_id"].as_str().unwrap_or_default();
                let Some(name) = names.get(id) else { continue };
                let content = m["content"].as_str().unwrap_or_default();
                contents.push(json!({ "role": "user", "parts": [{ "functionResponse": { "name": name, "response": { "content": content } } }] }));
            }
            _ => contents.push(json!({ "role": "user", "parts": [{ "text": m["content"].as_str().unwrap_or_default() }] })),
        }
    }

    let system: String = messages
        .iter()
        .filter(|m| m["role"] == "system")
        .filter_map(|m| m["content"].as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut request = json!({
        "contents": contents,
        "generationConfig": {
            "maxOutputTokens": MAX_OUTPUT,
            "thinkingConfig": { "thinkingBudget": effort.gemini_budget() },
        },
    });
    if !system.is_empty() {
        request["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }
    if let Some(tools) = tools {
        let declarations: Vec<Value> = tools
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| {
                json!({
                    "name": t["function"]["name"],
                    "description": t["function"]["description"],
                    "parameters": t["function"]["parameters"],
                })
            })
            .collect();
        if !declarations.is_empty() {
            request["tools"] = json!([{ "functionDeclarations": declarations }]);
        }
    }
    gemini::envelope(project, model, request)
}

fn call_names(messages: &[Value]) -> HashMap<String, String> {
    messages
        .iter()
        .filter_map(|m| m["tool_calls"].as_array())
        .flatten()
        .filter_map(|c| Some((c["id"].as_str()?.to_owned(), c["function"]["name"].as_str()?.to_owned())))
        .collect()
}

fn arguments(raw: &Value) -> Value {
    raw.as_str()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// Accepts both the wrapped (`{"response": …}`) and bare shapes.
pub fn unwrap(v: &Value) -> &Value {
    if v.get("response").is_some_and(Value::is_object) { &v["response"] } else { v }
}

pub fn apply(out: &mut Completion, data: &str, on: &mut impl FnMut(Delta<'_>)) -> Result<()> {
    let raw: Value = serde_json::from_str(data).with_context(|| format!("flux invalide: {data:.160}"))?;
    let v = unwrap(&raw);
    if let Some(u) = v.get("usageMetadata") {
        out.usage.prompt = u["promptTokenCount"].as_u64().unwrap_or(out.usage.prompt);
        out.usage.completion = u["candidatesTokenCount"].as_u64().unwrap_or(out.usage.completion);
        out.usage.cached = u["cachedContentTokenCount"].as_u64().unwrap_or(out.usage.cached);
    }
    for candidate in v["candidates"].as_array().into_iter().flatten() {
        for part in candidate["content"]["parts"].as_array().into_iter().flatten() {
            if let Some(t) = part["text"].as_str().filter(|t| !t.is_empty()) {
                on(Delta::Text(t));
                out.content.push_str(t);
            }
            let call = &part["functionCall"];
            if let Some(name) = call["name"].as_str() {
                out.tool_calls.push(ToolCall {
                    id: format!("gemini-{}", out.tool_calls.len()),
                    name: name.to_owned(),
                    arguments: call["args"].to_string(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_maps_to_contents_and_function_calls() {
        let msgs = vec![
            json!({ "role": "system", "content": "SYS" }),
            json!({ "role": "user", "content": "hi" }),
            json!({ "role": "assistant", "content": "", "tool_calls": [
                { "id": "c1", "type": "function", "function": { "name": "grep", "arguments": "{\"pattern\":\"x\"}" } } ]}),
            json!({ "role": "tool", "tool_call_id": "c1", "content": "1 hit" }),
        ];
        let b = body(&msgs, None, "gemini-2.5-pro", "p1", Effort::High);
        assert_eq!(b["model"], "gemini-2.5-pro");
        assert_eq!(b["project"], "p1");
        assert_eq!(b["request"]["systemInstruction"]["parts"][0]["text"], "SYS");
        assert_eq!(b["request"]["contents"][1]["role"], "model");
        assert_eq!(b["request"]["contents"][1]["parts"][0]["functionCall"]["name"], "grep");
        assert_eq!(b["request"]["contents"][2]["parts"][0]["functionResponse"]["name"], "grep");
        assert_eq!(b["request"]["generationConfig"]["thinkingConfig"]["thinkingBudget"], 8192);
    }

    #[test]
    fn tool_response_without_name_is_skipped() {
        let msgs = vec![json!({ "role": "tool", "tool_call_id": "gone", "content": "x" })];
        let b = body(&msgs, None, "m", "p", Effort::Off);
        assert_eq!(b["request"]["contents"].as_array().unwrap().len(), 0);
        assert_eq!(b["request"]["generationConfig"]["thinkingConfig"]["thinkingBudget"], 0);
    }

    #[test]
    fn streaming_parts_and_usage_are_read() {
        let mut out = Completion::default();
        for e in [
            r#"{"response":{"candidates":[{"content":{"parts":[{"text":"ok"}]}}]}}"#,
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"grep","args":{"pattern":"x"}}}]}}]}"#,
            r#"{"response":{"candidates":[{"content":{"parts":[]}}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":4,"cachedContentTokenCount":9}}}"#,
        ] {
            apply(&mut out, e, &mut |_| {}).unwrap();
        }
        assert_eq!(out.content, "ok");
        assert_eq!(out.tool_calls[0].name, "grep");
        assert_eq!(out.tool_calls[0].arguments, r#"{"pattern":"x"}"#);
        assert_eq!((out.usage.prompt, out.usage.completion, out.usage.cached), (12, 4, 9));
        assert!(out.tool_calls[0].id.starts_with("gemini-"));
    }
}
