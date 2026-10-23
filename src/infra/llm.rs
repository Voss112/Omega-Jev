//! System Two streaming client (Kimi K3.1 Pro / DeepSeek V4.1 Flash): OpenAI-compatible chat, tools, reasoning_content.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::domain::{
    chat::{ToolCall, Usage},
    engine::Engine,
    settings::Settings,
};

/// Output ceiling per request (reasoning included); a safety cap, not a target.
const MAX_OUTPUT: u32 = 65_536;

pub enum Delta<'a> {
    Text(&'a str),
    Reasoning(&'a str),
}

#[derive(Default)]
pub struct Completion {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
}

#[derive(Clone)]
pub struct Llm {
    http: Client,
    pub engine: Engine,
    base: String,
    key: String,
    pub model: String,
}

impl Llm {
    /// Endpoint of `engine` from the settings; empty key falls back to the engine's environment variable.
    pub fn new(http: Client, settings: &Settings, engine: Engine) -> Self {
        let (base, key, model) = settings.endpoint(engine);
        let key = if key.trim().is_empty() { std::env::var(engine.key_var()).unwrap_or_default() } else { key.trim().to_owned() };
        Self { http, engine, base: base.trim().trim_end_matches('/').into(), key, model: model.trim().into() }
    }

    /// A key is set and will only travel over TLS.
    pub fn configured(&self) -> bool {
        !self.key.is_empty() && self.base.starts_with("https://")
    }

    /// Streaming request body with the engine's fixed effort. Kimi: `temperature`/`top_p` are server-fixed, omitted.
    pub fn body(&self, messages: &[Value], tools: Option<Value>) -> Value {
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
            "reasoning_effort": self.engine.effort(),
        });
        match self.engine {
            Engine::Kimi => body["max_completion_tokens"] = json!(MAX_OUTPUT),
            Engine::DeepSeek => {
                body["max_tokens"] = json!(MAX_OUTPUT);
                body["thinking"] = json!({ "type": "enabled" });
            }
        }
        if let Some(tools) = tools {
            body["tools"] = tools;
        }
        body
    }

    /// Model ids this key can use.
    pub async fn models(&self) -> Result<Vec<String>> {
        let res =
            self.http.get(format!("{}/models", self.base)).bearer_auth(&self.key).timeout(Duration::from_secs(10)).send().await?;
        if !res.status().is_success() {
            bail!("HTTP {}", res.status());
        }
        let v: Value = res.json().await?;
        Ok(v["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(str::to_owned)).collect())
    }

    /// Key valid and configured model actually available to it.
    pub async fn ping(&self) -> Result<()> {
        let models = self.models().await?;
        anyhow::ensure!(
            models.iter().any(|m| m == &self.model),
            "{} non disponible sur ce compte (accès : {})",
            self.model,
            models.join(", ")
        );
        Ok(())
    }

    pub async fn stream(&self, body: &Value, mut on: impl FnMut(Delta<'_>)) -> Result<Completion> {
        let res = self.send(body).await?;
        let mut out = Completion::default();
        let mut buf: Vec<u8> = Vec::new();
        let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk.with_context(|| format!("flux {} interrompu", self.engine.label()))?);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                let Some(data) = line.trim().strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" {
                    return Ok(out);
                }
                apply(&mut out, data, &mut on)?;
            }
        }
        Ok(out)
    }

    async fn send(&self, body: &Value) -> Result<reqwest::Response> {
        anyhow::ensure!(
            self.base.starts_with("https://"),
            "URL {} non HTTPS refusée (la clé partirait en clair)",
            self.engine.label()
        );
        let url = format!("{}/chat/completions", self.base);
        let mut attempt = 0;
        loop {
            let res = self
                .http
                .post(&url)
                .bearer_auth(&self.key)
                .json(body)
                .send()
                .await
                .with_context(|| format!("{} injoignable", self.engine.label()))?;
            let status = res.status();
            if status.is_success() {
                return Ok(res);
            }
            let retry = status.as_u16() == 429 || status.is_server_error();
            let text = res.text().await.unwrap_or_default();
            attempt += 1;
            if status.as_u16() == 404 {
                let models = self.models().await.map_or_else(|_| "?".to_owned(), |m| m.join(", "));
                bail!("{} 404: {} — modèles accessibles avec cette clé : {models}", self.engine.label(), api_error(&text));
            }
            if !retry || attempt == 3 {
                bail!("{} {status}: {}", self.engine.label(), api_error(&text));
            }
            tokio::time::sleep(Duration::from_millis(900 * attempt)).await;
        }
    }
}

fn api_error(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.chars().take(300).collect())
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

/// Usage with every known cached-token field (OpenAI `prompt_tokens_details`, Moonshot `cached_tokens`, DeepSeek-style).
fn usage(u: &Value) -> Usage {
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
    }
}

fn apply(out: &mut Completion, data: &str, on: &mut impl FnMut(Delta<'_>)) -> Result<()> {
    let chunk: Chunk = serde_json::from_str(data).with_context(|| format!("flux invalide: {data:.160}"))?;
    if let Some(u) = chunk.usage.as_ref().or_else(|| chunk.choices.iter().find_map(|c| c.usage.as_ref())) {
        out.usage = usage(u);
    }
    for choice in chunk.choices {
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
    fn bodies_carry_each_engine_fixed_effort() {
        let (http, settings) = (crate::infra::http::client("").unwrap(), Settings::default());
        let msg = [json!({ "role": "user", "content": "x" })];
        let kimi = Llm::new(http.clone(), &settings, Engine::Kimi).body(&msg, None);
        assert_eq!((kimi["model"].as_str(), kimi["reasoning_effort"].as_str()), (Some("kimi-k3.1-pro"), Some("low")));
        assert!(kimi.get("temperature").is_none() && kimi.get("thinking").is_none() && kimi.get("tools").is_none());
        let ds = Llm::new(http, &settings, Engine::DeepSeek).body(&msg, None);
        assert_eq!((ds["model"].as_str(), ds["reasoning_effort"].as_str()), (Some("deepseek-flash"), Some("max")));
        assert_eq!(ds["thinking"]["type"], "enabled");
        let openai = usage(&json!({ "prompt_tokens": 10, "prompt_tokens_details": { "cached_tokens": 7 } }));
        assert_eq!(openai.cached, 7);
    }

    /// Real streamed call through this client: `MOONSHOT_API_KEY=… OMEGA_TEST_MODEL=… cargo test -- --ignored live_stream`.
    #[tokio::test]
    #[ignore = "network + API key"]
    async fn live_stream() {
        let model = std::env::var("OMEGA_TEST_MODEL").unwrap_or_else(|_| "kimi-k3.1-pro".into());
        let settings = Settings { kimi_model: model, ..Settings::default() };
        let llm = Llm::new(crate::infra::http::client("").unwrap(), &settings, Engine::Kimi);
        let body = llm.body(&[json!({ "role": "user", "content": "Reply with the single word: ok" })], None);
        let done = llm.stream(&body, |_| {}).await.unwrap();
        assert!(done.content.to_lowercase().contains("ok"), "content: {}", done.content);
        assert!(done.usage.prompt > 0 && done.usage.completion > 0, "usage parsed");
    }
}
