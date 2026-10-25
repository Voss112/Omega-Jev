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
            "system" | "tools" => {}
            "assistant" => {
                let mut parts: Vec<Value> = Vec::new();
                if let Some(text) = m["content"].as_str().filter(|t| !t.is_empty()) {
                    parts.push(json!({ "text": text }));
                }
                for call in m["tool_calls"].as_array().into_iter().flatten() {
                    let mut part = json!({
                        "functionCall": {
                            "name": call["function"]["name"],
                            "args": super::arguments(&call["function"]["arguments"]),
                        }
                    });
                    if let Some(sig) = call["signature"].as_str().filter(|s| !s.is_empty()) {
                        part["thoughtSignature"] = json!(sig);
                    }
                    parts.push(part);
                }
                if !parts.is_empty() {
                    contents.push(json!({ "role": "model", "parts": parts }));
                }
            }
            "tool" => {
                let id = m["tool_call_id"].as_str().unwrap_or_default();
                let Some(name) = names.get(id) else { continue };
                let content = m["content"].as_str().unwrap_or_default();
                let part = json!({ "functionResponse": { "name": name, "response": { "content": content } } });
                // Parallel calls answer in one user turn, as Gemini expects.
                match contents.last_mut() {
                    Some(last) if last["role"] == "user" && last["parts"][0].get("functionResponse").is_some() => {
                        if let Some(parts) = last["parts"].as_array_mut() {
                            parts.push(part);
                        }
                    }
                    _ => contents.push(json!({ "role": "user", "parts": [part] })),
                }
            }
            _ => contents.push(json!({ "role": "user", "parts": user_parts(&m["content"]) })),
        }
    }

    let system: String = messages
        .iter()
        .filter(|m| m["role"] == "system")
        .filter_map(|m| m["content"].as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut thinking = json!({ "thinkingBudget": effort.gemini_budget() });
    if effort != Effort::Off {
        thinking["includeThoughts"] = json!(true);
    }
    let mut request = json!({
        "contents": contents,
        "generationConfig": { "maxOutputTokens": MAX_OUTPUT, "thinkingConfig": thinking },
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
                let mut declaration = json!({ "name": t["function"]["name"], "description": t["function"]["description"] });
                // Gemini rejects an object schema without properties: a tool without arguments sends none.
                if !super::openai::no_parameters(&t["function"]["parameters"]) {
                    declaration["parameters"] = t["function"]["parameters"].clone();
                }
                declaration
            })
            .collect();
        if !declarations.is_empty() {
            request["tools"] = json!([{ "functionDeclarations": declarations }]);
        }
    }
    gemini::envelope(project, model, request)
}

/// Canonical (OpenAI-shaped) user content → Gemini parts; `data:` URLs become inline images.
fn user_parts(content: &Value) -> Vec<Value> {
    let Some(parts) = content.as_array() else {
        return vec![json!({ "text": content.as_str().unwrap_or_default() })];
    };
    parts
        .iter()
        .filter_map(|p| match p["type"].as_str().unwrap_or_default() {
            "text" => Some(json!({ "text": p["text"].as_str().unwrap_or_default() })),
            "image_url" => {
                let (media, data) = super::data_url(p["image_url"]["url"].as_str()?)?;
                Some(json!({ "inlineData": { "mimeType": media, "data": data } }))
            }
            _ => None,
        })
        .collect()
}

fn call_names(messages: &[Value]) -> HashMap<String, String> {
    messages
        .iter()
        .filter_map(|m| m["tool_calls"].as_array())
        .flatten()
        .filter_map(|c| Some((c["id"].as_str()?.to_owned(), c["function"]["name"].as_str()?.to_owned())))
        .collect()
}

/// Accepts both the wrapped (`{"response": …}`) and bare shapes.
pub fn unwrap(v: &Value) -> &Value {
    if v.get("response").is_some_and(Value::is_object) { &v["response"] } else { v }
}

pub fn apply(out: &mut Completion, data: &str, on: &mut impl FnMut(Delta<'_>)) -> Result<()> {
    let raw: Value = serde_json::from_str(data).with_context(|| format!("flux invalide: {data:.160}"))?;
    if let Some(message) = super::stream_error(&raw) {
        anyhow::bail!("erreur du fournisseur pendant le flux : {message}");
    }
    let v = unwrap(&raw);
    if let Some(u) = v.get("usageMetadata") {
        out.usage.prompt = u["promptTokenCount"].as_u64().unwrap_or(out.usage.prompt);
        let thoughts = u["thoughtsTokenCount"].as_u64().unwrap_or(0);
        out.usage.completion = u["candidatesTokenCount"].as_u64().map_or(out.usage.completion, |c| c + thoughts);
        out.usage.cached = u["cachedContentTokenCount"].as_u64().unwrap_or(out.usage.cached);
    }
    if let Some(reason) = v["promptFeedback"]["blockReason"].as_str() {
        out.stop = "content_filter".to_owned();
        out.stop_detail = reason.to_owned();
    }
    for candidate in v["candidates"].as_array().into_iter().flatten() {
        if let Some(reason) = candidate["finishReason"].as_str() {
            out.stop = stop(reason).to_owned();
            if out.stop == "content_filter" {
                out.stop_detail = reason.to_owned();
            }
        }
        for part in candidate["content"]["parts"].as_array().into_iter().flatten() {
            let signature = part["thoughtSignature"].as_str().unwrap_or_default();
            if let Some(t) = part["text"].as_str().filter(|t| !t.is_empty()) {
                if part["thought"].as_bool() == Some(true) {
                    on(Delta::Reasoning(t));
                    out.reasoning.push_str(t);
                } else {
                    on(Delta::Text(t));
                    out.content.push_str(t);
                }
            }
            let call = &part["functionCall"];
            if let Some(name) = call["name"].as_str() {
                out.tool_calls.push(ToolCall {
                    id: format!("gemini-{}", uuid::Uuid::new_v4().simple()),
                    name: name.to_owned(),
                    arguments: call["args"].to_string(),
                    signature: signature.to_owned(),
                });
            }
        }
    }
    Ok(())
}

/// Gemini finish reasons in the vocabulary `Completion::finish` understands.
fn stop(reason: &str) -> &str {
    match reason {
        "MAX_TOKENS" => "length",
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" => "content_filter",
        _ => "stop",
    }
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
                { "id": "c1", "type": "function", "function": { "name": "grep", "arguments": "{\"pattern\":\"x\"}" }, "signature": "sig1" },
                { "id": "c2", "type": "function", "function": { "name": "outline", "arguments": "{}" } } ]}),
            json!({ "role": "tool", "tool_call_id": "c1", "content": "1 hit" }),
            json!({ "role": "tool", "tool_call_id": "c2", "content": "fn a" }),
        ];
        let b = body(&msgs, None, "gemini-3.8-pro", "p1", Effort::High);
        assert_eq!(b["model"], "gemini-3.8-pro");
        assert_eq!(b["project"], "p1");
        let r = &b["request"];
        assert_eq!(r["systemInstruction"]["parts"][0]["text"], "SYS");
        assert_eq!(r["contents"][1]["role"], "model");
        assert_eq!(r["contents"][1]["parts"][0]["functionCall"]["name"], "grep");
        assert_eq!(r["contents"][1]["parts"][0]["thoughtSignature"], "sig1", "signature renvoyée telle quelle");
        assert!(r["contents"][1]["parts"][1].get("thoughtSignature").is_none());
        let responses = r["contents"][2]["parts"].as_array().unwrap();
        assert_eq!(responses.len(), 2, "réponses parallèles dans un seul tour");
        assert_eq!(r["generationConfig"]["thinkingConfig"]["thinkingBudget"], 8192);
        assert_eq!(r["generationConfig"]["thinkingConfig"]["includeThoughts"], true);
        let tools = json!([
            { "type": "function", "function": { "name": "audit_kit", "description": "d", "parameters": { "type": "object", "properties": {}, "required": [] } } },
            { "type": "function", "function": { "name": "grep", "description": "d", "parameters": { "type": "object", "properties": { "p": { "type": "string" } } } } },
        ]);
        let declared = &body(&msgs, Some(tools), "m", "p", Effort::Low)["request"]["tools"][0]["functionDeclarations"];
        assert!(declared[0].get("parameters").is_none(), "sans argument : pas de schéma vide");
        assert!(declared[1].get("parameters").is_some());
    }

    #[test]
    fn tool_response_without_name_is_skipped() {
        let msgs = vec![json!({ "role": "tool", "tool_call_id": "gone", "content": "x" })];
        let b = body(&msgs, None, "m", "p", Effort::Off);
        assert_eq!(b["request"]["contents"].as_array().unwrap().len(), 0);
        assert_eq!(b["request"]["generationConfig"]["thinkingConfig"]["thinkingBudget"], 0);
        assert!(b["request"]["generationConfig"]["thinkingConfig"].get("includeThoughts").is_none());
    }

    #[test]
    fn a_data_url_becomes_an_inline_image_part() {
        let msgs = vec![json!({ "role": "user", "content": [
            { "type": "text", "text": "regarde" },
            { "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,QUJD" } },
        ]})];
        let b = body(&msgs, None, "m", "p", Effort::Off);
        let parts = &b["request"]["contents"][0]["parts"];
        assert_eq!(parts[0]["text"], "regarde");
        assert_eq!(parts[1]["inlineData"]["mimeType"], "image/jpeg");
        assert_eq!(parts[1]["inlineData"]["data"], "QUJD");
    }

    #[test]
    fn streaming_parts_signatures_thoughts_and_usage_are_read() {
        let mut out = Completion::default();
        for e in [
            r#"{"response":{"candidates":[{"content":{"parts":[{"text":"plan","thought":true}]}}]}}"#,
            r#"{"response":{"candidates":[{"content":{"parts":[{"text":"Voici"}]}}]}}"#,
            r#"{"response":{"candidates":[{"content":{"parts":[{"functionCall":{"name":"grep","args":{"pattern":"x"}},"thoughtSignature":"s1"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":4,"thoughtsTokenCount":3}}}"#,
        ] {
            apply(&mut out, e, &mut |_| {}).unwrap();
        }
        assert_eq!((out.reasoning.as_str(), out.content.as_str()), ("plan", "Voici"));
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!((out.tool_calls[0].name.as_str(), out.tool_calls[0].signature.as_str()), ("grep", "s1"));
        assert_eq!(out.tool_calls[0].arguments, r#"{"pattern":"x"}"#);
        assert_eq!((out.usage.prompt, out.usage.completion), (10, 7));
    }

    #[test]
    fn a_blocked_answer_is_reported_as_filtered() {
        let mut out = Completion::default();
        apply(&mut out, r#"{"response":{"candidates":[{"finishReason":"SAFETY"}]}}"#, &mut |_| {}).unwrap();
        assert_eq!((out.stop.as_str(), out.stop_detail.as_str()), ("content_filter", "SAFETY"));
        let mut long = Completion::default();
        apply(&mut long, r#"{"candidates":[{"finishReason":"MAX_TOKENS","content":{"parts":[{"text":"a"}]}}]}"#, &mut |_| {})
            .unwrap();
        assert_eq!(long.stop, "length");
        assert!(apply(&mut Completion::default(), r#"{"error":{"message":"quota"}}"#, &mut |_| {}).is_err());
    }
}
