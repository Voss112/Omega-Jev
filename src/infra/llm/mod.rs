//! System Two streaming client: one facade over three wire protocols
//! (OpenAI-compatible, Anthropic Messages, Gemini Code Assist) with subscription tokens.

mod anthropic;
mod gemini;
mod openai;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder};
use serde_json::{Value, json};

use crate::domain::{
    auth::{AuthMode, Provider},
    chat::{ToolCall, Usage},
    engine::{Effort, Engine, Wire},
    settings::Settings,
};
use crate::infra::auth::{Auth, claude, gemini as gemini_auth};

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
    /// Anthropic content-block index → position in `tool_calls`.
    #[doc(hidden)]
    pub blocks: std::collections::BTreeMap<usize, usize>,
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
    /// Gemini Code Assist project id.
    project: String,
}

impl Llm {
    /// Endpoint of `engine` from the settings; empty key falls back to the engine's environment variable.
    pub fn new(http: Client, settings: &Settings, engine: Engine) -> Self {
        let (base, key, model) = settings.endpoint(engine);
        let key = if key.trim().is_empty() { std::env::var(engine.key_var()).unwrap_or_default() } else { key.trim().to_owned() };
        let provider = match engine {
            Engine::Claude => Some(Provider::Claude),
            Engine::Gemini => Some(Provider::Gemini),
            Engine::Kimi | Engine::DeepSeek => None,
        };
        let auth = provider.filter(|_| settings.auth_mode(engine) == AuthMode::Subscription).and_then(Auth::load);
        // Subscription endpoints are the providers' own: the API-key URL never carries a subscription token.
        let base = match (engine, auth.is_some()) {
            (Engine::Claude, true) => claude::API.to_owned(),
            (Engine::Gemini, true) => gemini_auth::CODE_ASSIST.to_owned(),
            _ => base.trim().trim_end_matches('/').to_owned(),
        };
        let project = auth.as_ref().map(|a| a.project()).unwrap_or_default();
        Self { http, engine, base, key, model: model.trim().into(), effort: settings.effort.get(engine), auth, project }
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

    /// Streaming request body in the engine's native shape.
    pub fn body(&self, messages: &[Value], tools: Option<Value>) -> Value {
        match self.engine.wire() {
            Wire::OpenAi => openai::body(&self.model, self.engine, self.effort, messages, tools),
            Wire::Anthropic => anthropic::body(messages, tools, &self.model, self.effort, self.auth.is_some()),
            Wire::Gemini => gemini::body(messages, tools, &self.model, &self.project(), self.effort),
        }
    }

    /// Code Assist project: the stored one wins, onboarding may have attached a new one.
    fn project(&self) -> String {
        self.auth.as_ref().map(|a| a.project()).filter(|p| !p.is_empty()).unwrap_or_else(|| self.project.clone())
    }

    /// Model ids this credential can use.
    pub async fn models(&self) -> Result<Vec<String>> {
        anyhow::ensure!(self.base.starts_with("https://"), "URL {} non HTTPS refusée", self.engine.label());
        let (url, oauth) = match self.engine.wire() {
            Wire::Gemini => return Ok(Vec::new()),
            Wire::OpenAi => (format!("{}/models", self.base), false),
            Wire::Anthropic => (format!("{}/v1/models", self.base), self.auth.is_some()),
        };
        let token = self.authorization().await?;
        let req = self.http.get(&url).timeout(Duration::from_secs(10));
        let req =
            if self.engine.wire() == Wire::Anthropic { anthropic_headers(req, &token, oauth) } else { req.bearer_auth(&token) };
        let res = req.send().await.with_context(|| format!("{} injoignable", self.engine.label()))?;
        if !res.status().is_success() {
            bail!("HTTP {}", res.status());
        }
        let v: Value = res.json().await?;
        Ok(v["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(str::to_owned)).collect())
    }

    /// Credential valid and configured model actually reachable.
    pub async fn ping(&self) -> Result<()> {
        if self.engine.wire() == Wire::Gemini {
            let auth = self.auth.as_ref().with_context(|| self.missing())?;
            let token = auth.fresh(&self.http).await?;
            let (tier, project) = gemini_auth::tier(&self.http, &token, &self.project()).await?;
            anyhow::ensure!(!project.is_empty(), "Code Assist : aucun projet attaché (offre {tier})");
            if project != auth.project() {
                auth.save(crate::domain::auth::Cred { project, ..auth.cred() })?;
            }
            return Ok(());
        }
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
        let body = json!({ "model": self.model, "max_tokens": 1, "messages": [ { "role": "user", "content": "ok" } ] });
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
        while let Some(chunk) = stream.next().await {
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
                    return Ok(out);
                }
                self.apply(&mut out, data, &mut on)?;
            }
        }
        Ok(out)
    }

    fn apply(&self, out: &mut Completion, data: &str, on: &mut impl FnMut(Delta<'_>)) -> Result<()> {
        match self.engine.wire() {
            Wire::OpenAi => openai::apply(out, data, on),
            Wire::Anthropic => anthropic::apply(out, data, on),
            Wire::Gemini => gemini::apply(out, data, on),
        }
    }

    fn url(&self) -> String {
        match self.engine.wire() {
            Wire::OpenAi => format!("{}/chat/completions", self.base),
            Wire::Anthropic => format!("{}/v1/messages", self.base),
            Wire::Gemini => format!("{}/:streamGenerateContent?alt=sse", self.base),
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
            req = match self.engine.wire() {
                Wire::OpenAi | Wire::Gemini => req.bearer_auth(&token),
                Wire::Anthropic => anthropic_headers(req, &token, self.auth.is_some()),
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
                if code == 404 && self.engine.wire() == Wire::OpenAi {
                    let models = self.models().await.map_or_else(|_| "?".to_owned(), |m| m.join(", "));
                    bail!("{} 404: {} — modèles accessibles avec cette clé : {models}", self.engine.label(), error_text(&text));
                }
                if self.engine.wire() == Wire::Gemini
                    && let Some(hint) = gemini_auth::hint(&text)
                {
                    bail!("{} {status}: {hint}", self.engine.label());
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

fn anthropic_headers(req: RequestBuilder, token: &str, oauth: bool) -> RequestBuilder {
    let req = req.header("anthropic-version", claude::VERSION);
    if oauth {
        req.header("authorization", format!("Bearer {token}")).header("anthropic-beta", claude::BETA)
    } else {
        req.header("x-api-key", token)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_carry_the_selected_effort() {
        let (http, mut settings) = (crate::infra::http::client("").unwrap(), Settings::default());
        let msg = [json!({ "role": "user", "content": "x" })];
        settings.effort.set(Engine::Kimi, Effort::High);
        let kimi = Llm::new(http.clone(), &settings, Engine::Kimi).body(&msg, None);
        assert_eq!((kimi["model"].as_str(), kimi["reasoning_effort"].as_str()), (Some("kimi-k3.1-pro"), Some("high")));
        assert!(kimi.get("temperature").is_none() && kimi.get("thinking").is_none() && kimi.get("tools").is_none());
        settings.effort.set(Engine::Kimi, Effort::Off);
        assert!(Llm::new(http.clone(), &settings, Engine::Kimi).body(&msg, None).get("reasoning_effort").is_none());
        let ds = Llm::new(http, &settings, Engine::DeepSeek).body(&msg, None);
        assert_eq!((ds["model"].as_str(), ds["reasoning_effort"].as_str()), (Some("deepseek-flash"), Some("max")));
        assert_eq!(ds["thinking"]["type"], "enabled");
    }

    #[test]
    fn api_key_mode_without_a_key_is_not_configured() {
        let http = crate::infra::http::client("").unwrap();
        for (engine, mode) in [(Engine::Claude, AuthMode::ApiKey), (Engine::Gemini, AuthMode::ApiKey)] {
            let s = match engine {
                Engine::Claude => Settings { claude_auth: mode, claude_key: String::new(), ..Settings::default() },
                _ => Settings { gemini_auth: mode, gemini_key: String::new(), ..Settings::default() },
            };
            let llm = Llm::new(http.clone(), &s, engine);
            // Subscription credentials on disk must not be mistaken for an API key.
            assert!(!llm.subscription(), "{engine:?}");
            assert!(!llm.configured(), "{engine:?} sans clé");
        }
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
