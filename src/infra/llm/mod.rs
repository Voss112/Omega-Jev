//! System Two streaming client: one facade over two wire protocols
//! (OpenAI-compatible, Anthropic Messages) with subscription tokens.

mod anthropic;
mod openai;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::domain::{
    auth::{AuthMode, Provider},
    chat::{Part, ToolCall, Usage},
    engine::{Effort, Engine, Wire},
    models::Choice,
    settings::Settings,
};
use crate::infra::auth::{Auth, claude};

pub enum Delta<'a> {
    Text(&'a str),
    Reasoning(&'a str),
}

#[derive(Default)]
pub struct Completion {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    /// Claude: every block in stream order, replayed unchanged by the tool loop.
    pub parts: Vec<Part>,
    pub usage: Usage,
    /// Why generation stopped, in the provider's words (`refusal`, `SAFETY`, `length`…).
    pub stop: String,
    /// Refusal or block detail, when the provider gives one.
    pub stop_detail: String,
    /// Anthropic content-block index → position in `parts`.
    #[doc(hidden)]
    pub blocks: std::collections::BTreeMap<usize, usize>,
}

impl Completion {
    /// Turns a refused or blocked stream into an error, drops nameless calls and gives every call an id.
    fn finish(mut self, engine: Engine) -> Result<Self> {
        let detail = if self.stop_detail.is_empty() { self.stop.clone() } else { self.stop_detail.clone() };
        if self.stop == "refusal" {
            bail!("{} a refusé de répondre ({detail}) — reformulez la demande.", engine.label());
        }
        if self.parts.is_empty() {
            self.tool_calls.retain(|c| !c.name.trim().is_empty());
        }
        let empty = self.content.trim().is_empty() && self.tool_calls.is_empty();
        if empty && BLOCKED.contains(&self.stop.as_str()) {
            bail!("{} a interrompu la réponse ({detail}).", engine.label());
        }
        for call in &mut self.tool_calls {
            if call.id.trim().is_empty() {
                call.id = format!("call_{}", Uuid::new_v4().simple());
            }
        }
        if self.tool_calls.is_empty() && ["max_tokens", "length"].contains(&self.stop.as_str()) {
            self.content.push_str("\n\n[réponse tronquée : limite de sortie atteinte — envoyez « continue »]");
        }
        Ok(self)
    }
}

/// Stop reasons that mean the provider withheld the answer.
const BLOCKED: &[&str] = &["content_filter"];

/// A 400 for a replayed Claude thinking block whose conversation changed: retry without the blocks.
pub fn thinking_rejected(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}");
    text.contains("signature") && text.contains("thinking")
}

pub use anthropic::uses_deferred_tools;

/// A 400 about mid-conversation tool changes (model, account or beta not enabled): retry with classic tools.
pub fn tools_beta_rejected(error: &anyhow::Error) -> bool {
    const MARKERS: &[&str] =
        &["tool_addition", "tool_reference", "defer_loading", "mid-conversation", "inline-tools", "beta", "role \"system\"", "system message"];
    let text = format!("{error:#}").to_lowercase();
    text.contains("400") && MARKERS.iter().any(|m| text.contains(m))
}

#[derive(Clone)]
pub struct Llm {
    http: Client,
    pub engine: Engine,
    base: String,
    key: String,
    pub model: String,
    pub effort: Effort,
    /// `Some` = consumer subscription; the token is refreshed on demand.
    auth: Option<std::sync::Arc<Auth>>,
    /// Claude deferred-tools mode still allowed (turned off for the session once the API refuses the beta).
    deferred: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Llm {
    /// Endpoint of `engine` from the settings; empty key falls back to the engine's environment variable.
    pub fn new(http: Client, settings: &Settings, engine: Engine) -> Self {
        let (base, key, model) = settings.endpoint(engine);
        let key = if key.trim().is_empty() { std::env::var(engine.key_var()).unwrap_or_default() } else { key.trim().to_owned() };
        let provider = match engine {
            Engine::Claude => Some(Provider::Claude),
            Engine::DeepSeek => None,
        };
        let auth = provider.filter(|_| settings.auth_mode(engine) == AuthMode::Subscription).and_then(Auth::load);
        // Subscription endpoints are the providers' own: the API-key URL never carries a subscription token.
        let base = match (engine, auth.is_some()) {
            (Engine::Claude, true) => claude::API.to_owned(),
            _ => base.trim().trim_end_matches('/').to_owned(),
        };
        Self {
            http,
            engine,
            base,
            key,
            model: model.trim().into(),
            effort: settings.effort.get(engine),
            auth,
            deferred: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }
    }

    pub fn wire(&self) -> Wire {
        self.engine.wire()
    }

    /// `tools` sent whole and constant, new ones surfaced mid-conversation: the cache and signed reasoning survive
    /// a turn that needs more tools.
    pub fn deferred_tools(&self) -> bool {
        self.wire() == Wire::Anthropic
            && anthropic::inline_tools(&self.model)
            && self.deferred.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn disable_deferred(&self) {
        self.deferred.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// A key is set (or a subscription is connected) and will only travel over TLS.
    pub fn configured(&self) -> bool {
        self.base.starts_with("https://") && (self.auth.is_some() || !self.key.is_empty())
    }

    pub fn subscription(&self) -> bool {
        self.auth.is_some()
    }

    pub fn account(&self) -> String {
        self.auth.as_ref().map(|a| a.account()).unwrap_or_default()
    }

    /// Ahead of a turn: TLS connection opened and a near-expiry subscription token refreshed off the critical path.
    pub async fn warm(&self) {
        if !self.configured() {
            return;
        }
        if self.auth.is_some() {
            let _ = self.authorization().await;
        }
        crate::infra::http::warm(&self.http, &self.base).await;
    }

    /// Streaming request body in the engine's native shape.
    pub fn body(&self, messages: &[Value], tools: Option<Value>) -> Value {
        match self.wire() {
            Wire::OpenAi => openai::body(&self.model, self.engine, self.effort, messages, tools),
            Wire::Anthropic => anthropic::body(messages, tools, &self.model, self.effort, self.auth.is_some()),
        }
    }

    /// Same endpoint and credential at another effort.
    pub fn with_effort(&self, effort: Effort) -> Self {
        Self { effort, ..self.clone() }
    }

    /// Exact prompt size of `body` as the provider counts it: Claude's free `count_tokens` endpoint,
    /// a 1-token completion on OpenAI-compatible engines (billed, negligible output).
    pub async fn count(&self, body: &Value) -> Result<u64> {
        match self.wire() {
            Wire::Anthropic => {
                anyhow::ensure!(self.base.starts_with("https://"), "URL {} non HTTPS refusée", self.engine.label());
                let shape: serde_json::Map<String, Value> = ["model", "system", "messages", "tools", "thinking"]
                    .into_iter()
                    .filter_map(|k| Some((k.to_owned(), body.get(k)?.clone())))
                    .collect();
                let token = self.authorization().await?;
                let req = self.http.post(format!("{}/v1/messages/count_tokens", self.base)).timeout(Duration::from_secs(30));
                let res = anthropic_headers(req.json(&shape), &token, self.auth.is_some(), anthropic::uses_deferred_tools(body))
                    .send()
                    .await
                    .with_context(|| format!("{} injoignable", self.engine.label()))?;
                let status = res.status();
                let text = res.text().await.unwrap_or_default();
                anyhow::ensure!(status.is_success(), "count_tokens {status}: {}", error_text(&text));
                serde_json::from_str::<Value>(&text)?["input_tokens"].as_u64().context("réponse sans input_tokens")
            }
            Wire::OpenAi => {
                let mut body = body.clone();
                body["max_tokens"] = json!(1);
                let done = self.stream(&body, |_| {}).await?;
                anyhow::ensure!(done.usage.prompt > 0, "{} n'a renvoyé aucun usage", self.engine.label());
                Ok(done.usage.prompt)
            }
        }
    }

    /// Model ids this credential can use.
    pub async fn models(&self) -> Result<Vec<String>> {
        Ok(self.listed().await?.into_iter().map(|c| c.id).collect())
    }

    /// Same list with display names, for the picker.
    pub async fn catalog(&self) -> Result<Vec<Choice>> {
        self.listed().await
    }

    /// Provider model list (`/models`, `/v1/models`), ids and display names.
    async fn listed(&self) -> Result<Vec<Choice>> {
        anyhow::ensure!(self.base.starts_with("https://"), "URL {} non HTTPS refusée", self.engine.label());
        let anthropic = self.wire() == Wire::Anthropic;
        let url = if anthropic { format!("{}/v1/models?limit=1000", self.base) } else { format!("{}/models", self.base) };
        let token = self.authorization().await?;
        let req = self.http.get(&url).timeout(Duration::from_secs(10));
        let req = if anthropic { anthropic_headers(req, &token, self.auth.is_some(), false) } else { req.bearer_auth(&token) };
        let res = req.send().await.with_context(|| format!("{} injoignable", self.engine.label()))?;
        if !res.status().is_success() {
            bail!("HTTP {}", res.status());
        }
        let v: Value = res.json().await?;
        let items = v["data"].as_array().or_else(|| v["models"].as_array());
        Ok(items.into_iter().flatten().filter_map(model_entry).collect())
    }

    /// Credential valid and configured model actually reachable.
    pub async fn ping(&self) -> Result<()> {
        match self.models().await {
            Ok(models) => {
                anyhow::ensure!(
                    models.iter().any(|m| m == &self.model),
                    "{} non disponible sur ce compte (accès : {})",
                    self.model,
                    models.join(", ")
                );
                Ok(())
            }
            // Subscription tokens do not always expose /v1/models: a 1-token message proves the token instead.
            Err(e) if self.auth.is_some() => self.probe().await.with_context(|| format!("{e:#}")),
            Err(e) => Err(e),
        }
    }

    async fn probe(&self) -> Result<()> {
        // The model's own request shape (thinking settings differ by generation), as small as it gets.
        let quiet = Self { effort: Effort::Off, ..self.clone() };
        let mut body = quiet.body(&[json!({ "role": "user", "content": "ok" })], None);
        body["max_tokens"] = json!(16);
        let res = self.send(&body).await?;
        let status = res.status();
        anyhow::ensure!(status.is_success(), "{} {status}", self.engine.label());
        Ok(())
    }

    pub async fn stream(&self, body: &Value, mut on: impl FnMut(Delta<'_>)) -> Result<Completion> {
        let res = self.send(body).await?;
        let mut out = Completion::default();
        let mut buf: Vec<u8> = Vec::new();
        let mut stream = res.bytes_stream();
        'read: while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk.with_context(|| format!("flux {} interrompu", self.engine.label()))?);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                let Some(data) = line.trim().strip_prefix("data:") else { continue };
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }
                if data == "[DONE]" {
                    break 'read;
                }
                self.apply(&mut out, data, &mut on)?;
            }
        }
        out.finish(self.engine)
    }

    fn apply(&self, out: &mut Completion, data: &str, on: &mut impl FnMut(Delta<'_>)) -> Result<()> {
        match self.wire() {
            Wire::OpenAi => openai::apply(out, data, on),
            Wire::Anthropic => anthropic::apply(out, data, on),
        }
    }

    fn url(&self) -> String {
        match self.wire() {
            Wire::OpenAi => format!("{}/chat/completions", self.base),
            Wire::Anthropic => format!("{}/v1/messages", self.base),
        }
    }

    async fn authorization(&self) -> Result<String> {
        match &self.auth {
            Some(auth) => auth.fresh(&self.http).await,
            None => Ok(self.key.clone()),
        }
    }

    fn missing(&self) -> String {
        format!("{} : abonnement non connecté — ouvrez ⚙ Paramètres.", self.engine.label())
    }

    async fn send(&self, body: &Value) -> Result<reqwest::Response> {
        anyhow::ensure!(
            self.base.starts_with("https://"),
            "URL {} non HTTPS refusée (la clé partirait en clair)",
            self.engine.label()
        );
        anyhow::ensure!(self.configured(), "{}", self.missing());
        let url = self.url();
        let mut attempt: u32 = 0;
        loop {
            let token = self.authorization().await?;
            let mut req = self.http.post(&url).json(body);
            req = match self.wire() {
                Wire::OpenAi => req.bearer_auth(&token),
                Wire::Anthropic => anthropic_headers(req, &token, self.auth.is_some(), anthropic::uses_deferred_tools(body)),
            };
            let res = req.send().await.with_context(|| format!("{} injoignable", self.engine.label()))?;
            let status = res.status();
            if status.is_success() {
                return Ok(res);
            }
            let code = status.as_u16();
            let text = res.text().await.unwrap_or_default();
            // A stale subscription token: refresh once, then retry immediately.
            if code == 401
                && attempt == 0
                && let Some(auth) = &self.auth
            {
                auth.replace(crate::domain::auth::Cred { expires_at: 0, ..auth.cred() });
                attempt += 1;
                continue;
            }
            attempt += 1;
            if !retryable(code, attempt) {
                if code == 404 && self.wire() == Wire::OpenAi {
                    let models = self.models().await.map_or_else(|_| "?".to_owned(), |m| m.join(", "));
                    bail!("{} 404: {} — modèles accessibles avec cette clé : {models}", self.engine.label(), error_text(&text));
                }
                bail!("{} {status}: {}", self.engine.label(), error_text(&text));
            }
            tokio::time::sleep(Duration::from_millis(900 * u64::from(attempt))).await;
        }
    }
}

/// Retry on rate limits and server errors, three attempts maximum.
fn retryable(status: u16, attempt: u32) -> bool {
    (status == 429 || (500..600).contains(&status)) && attempt < 3
}

/// One entry out of `{id, display_name}` (Anthropic), `{id}` (OpenAI) or `{name: "models/x", displayName}`.
fn model_entry(m: &Value) -> Option<Choice> {
    let raw = m["id"].as_str().or_else(|| m["name"].as_str())?.trim();
    let id = raw.strip_prefix("models/").unwrap_or(raw);
    if id.is_empty() {
        return None;
    }
    let name =
        m["display_name"].as_str().or_else(|| m["displayName"].as_str()).map(str::trim).filter(|n| !n.is_empty()).unwrap_or(id);
    Some(Choice::new(id, name))
}

fn anthropic_headers(req: RequestBuilder, token: &str, oauth: bool, deferred: bool) -> RequestBuilder {
    let req = req.header("anthropic-version", claude::VERSION);
    let betas = match (oauth, deferred) {
        (true, true) => Some(format!("{},{}", claude::BETA, anthropic::TOOL_CHANGES_BETA)),
        (true, false) => Some(claude::BETA.to_owned()),
        (false, true) => Some(anthropic::TOOL_CHANGES_BETA.to_owned()),
        (false, false) => None,
    };
    let req = match betas {
        Some(b) => req.header("anthropic-beta", b),
        None => req,
    };
    if oauth { req.header("authorization", format!("Bearer {token}")) } else { req.header("x-api-key", token) }
}

/// One readable line out of any provider error body.
pub fn error_text(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v["error"]["message"].as_str().or_else(|| v["error"].as_str()).or_else(|| v["message"].as_str()).map(str::to_owned)
        })
        .unwrap_or_else(|| body.chars().take(300).collect())
}

/// An error object sent inside the stream (HTTP 200) instead of an answer.
fn stream_error(v: &Value) -> Option<String> {
    let e = v.get("error")?;
    Some(e["message"].as_str().or_else(|| e.as_str()).map_or_else(|| e.to_string(), str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_carry_the_selected_effort() {
        let (http, mut settings) = (crate::infra::http::client("").unwrap(), Settings::default());
        let msg = [json!({ "role": "user", "content": "x" })];
        settings.effort.set(Engine::DeepSeek, Effort::High);
        let ds = Llm::new(http.clone(), &settings, Engine::DeepSeek).body(&msg, None);
        assert_eq!((ds["model"].as_str(), ds["reasoning_effort"].as_str()), (Some("deepseek-flash"), Some("high")));
        assert_eq!(ds["thinking"]["type"], "enabled");
        assert!(ds.get("tools").is_none());
    }

    #[test]
    fn api_key_mode_without_a_key_is_not_configured() {
        let http = crate::infra::http::client("").unwrap();
        let s = Settings { claude_auth: AuthMode::ApiKey, claude_key: String::new(), ..Settings::default() };
        let llm = Llm::new(http, &s, Engine::Claude);
        // Subscription credentials on disk must not be mistaken for an API key.
        assert!(!llm.subscription());
        assert!(!llm.configured(), "sans clé");
    }

    #[test]
    fn api_key_mode_selects_the_right_wire_and_retry_policy() {
        let s = Settings { engine: Engine::Claude, claude_auth: AuthMode::ApiKey, claude_key: "k".into(), ..Settings::default() };
        let llm = Llm::new(crate::infra::http::client("").unwrap(), &s, Engine::Claude);
        assert!(llm.configured() && !llm.subscription());
        assert_eq!(llm.url(), "https://api.anthropic.com/v1/messages");
        assert!(retryable(429, 1));
        assert!(!retryable(401, 1));
        assert!(!retryable(500, 3), "plafond de tentatives");
        assert_eq!(error_text(r#"{"error":{"message":"bad key"}}"#), "bad key");
        assert_eq!(error_text("<html>oops</html>"), "<html>oops</html>");
    }

    #[test]
    fn model_lists_accept_every_provider_shape() {
        let anthropic = json!({ "data": [{ "id": "claude-opus-5-5", "display_name": "Claude Opus 5.5" }] });
        assert_eq!(model_entry(&anthropic["data"][0]).unwrap().label(), "Claude Opus 5.5 · claude-opus-5-5");
        let prefixed = json!({ "id": "models/deepseek-flash" });
        assert_eq!(model_entry(&prefixed).unwrap().id, "deepseek-flash");
        assert!(model_entry(&json!({})).is_none());
    }

    #[test]
    fn switching_the_model_changes_the_wire_body() {
        let http = crate::infra::http::client("").unwrap();
        let msg = [json!({ "role": "user", "content": "x" })];
        let mut settings = Settings::default();
        settings.set_model(Engine::Claude, "claude-opus-5-5");
        let claude = Llm::new(http.clone(), &settings, Engine::Claude).body(&msg, None);
        assert_eq!(claude["model"], "claude-opus-5-5");
        settings.set_model(Engine::DeepSeek, "deepseek-pro");
        let ds = Llm::new(http, &settings, Engine::DeepSeek).body(&msg, None);
        assert_eq!(ds["model"], "deepseek-pro", "le corps porte l'id choisi");
    }

    #[test]
    fn a_refused_or_blocked_stream_is_an_error_not_an_empty_answer() {
        let refused = Completion { stop: "refusal".into(), content: "partial".into(), ..Completion::default() };
        assert!(refused.finish(Engine::Claude).err().is_some_and(|e| e.to_string().contains("refusé")));
        let blocked = Completion { stop: "content_filter".into(), ..Completion::default() };
        assert!(blocked.finish(Engine::DeepSeek).is_err());
        let truncated = Completion { stop: "length".into(), content: "abc".into(), ..Completion::default() };
        assert!(truncated.finish(Engine::DeepSeek).unwrap().content.contains("tronquée"));
        let calls = vec![ToolCall { name: "grep".into(), ..ToolCall::default() }, ToolCall::default()];
        let done = Completion { tool_calls: calls, ..Completion::default() }.finish(Engine::DeepSeek).unwrap();
        assert_eq!(done.tool_calls.len(), 1, "un appel sans nom est écarté");
        assert!(done.tool_calls[0].id.starts_with("call_"), "un appel sans id en reçoit un");
        assert!(thinking_rejected(&anyhow::anyhow!("Claude 400: messages.5.content.0: Invalid `signature` in `thinking` block")));
    }

    /// Real streamed call through this client: `DEEPSEEK_API_KEY=… OMEGA_TEST_MODEL=… cargo test -- --ignored live_stream`.
    #[tokio::test]
    #[ignore = "network + API key"]
    async fn live_stream() {
        let model = std::env::var("OMEGA_TEST_MODEL").unwrap_or_else(|_| "deepseek-flash".into());
        let settings = Settings { deepseek_model: model, ..Settings::default() };
        let llm = Llm::new(crate::infra::http::client("").unwrap(), &settings, Engine::DeepSeek);
        let body = llm.body(&[json!({ "role": "user", "content": "Reply with the single word: ok" })], None);
        let done = llm.stream(&body, |_| {}).await.unwrap();
        assert!(done.content.to_lowercase().contains("ok"), "content: {}", done.content);
        assert!(done.usage.prompt > 0 && done.usage.completion > 0, "usage parsed");
    }
}
