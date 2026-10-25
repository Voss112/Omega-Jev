//! System Two streaming client: one facade over three wire protocols
//! (OpenAI-compatible, Anthropic Messages, Gemini Code Assist) with subscription tokens.

pub mod agy;
mod anthropic;
mod gemini;
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
            let en = crate::ui::english();
            let hint = match (detail.as_str(), en) {
                ("reasoning_extraction", false) => {
                    "la demande lui fait recopier son raisonnement interne dans la réponse : retirez toute consigne sur sa façon de réfléchir"
                }
                ("reasoning_extraction", true) => {
                    "the request asks it to write out its internal reasoning: remove any instruction about how it should think"
                }
                ("cyber" | "bio" | "general_harms" | "frontier_llm", false) => "filtre de sécurité du fournisseur : reformulez ou changez de moteur",
                ("cyber" | "bio" | "general_harms" | "frontier_llm", true) => "provider safety filter: rephrase or switch engine",
                (_, false) => "reformulez la demande",
                (_, true) => "rephrase the request",
            };
            if en {
                bail!("{} declined to answer ({detail}) — {hint}.", engine.label());
            }
            bail!("{} a refusé de répondre ({detail}) — {hint}.", engine.label());
        }
        if self.parts.is_empty() {
            self.tool_calls.retain(|c| !c.name.trim().is_empty());
        }
        // A tool call written as text (DeepSeek's DSML markup) when no tool was offered is not an answer:
        // the next engine of the turn takes over instead of showing the markup.
        if self.tool_calls.is_empty() && call_markup(&self.content) {
            bail!("{} a écrit un appel d'outil en texte au lieu de répondre.", engine.label());
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

/// Raw tool-call markup a model emits when it wants a tool the request did not offer.
fn call_markup(content: &str) -> bool {
    content.contains("DSML") && content.contains("invoke name=")
        || content.contains("<｜tool▁calls▁begin｜>")
        || content.trim_start().starts_with("<function_calls>")
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
    /// Gemini subscription through the Antigravity CLI: the `agy` binary (text only).
    agy: Option<std::path::PathBuf>,
}

impl Llm {
    /// Endpoint of `engine` from the settings; empty key falls back to the engine's environment variable.
    pub fn new(http: Client, settings: &Settings, engine: Engine) -> Self {
        let (base, key, model) = settings.endpoint(engine);
        let key = if key.trim().is_empty() { std::env::var(engine.key_var()).unwrap_or_default() } else { key.trim().to_owned() };
        let subscribed = settings.auth_mode(engine) == AuthMode::Subscription;
        // Personal Google plans only reach Gemini through Antigravity now; Code Assist stays for Cloud projects.
        let agy = (engine == Engine::Gemini && subscribed).then(agy::binary).flatten();
        let auth = Provider::of(engine).filter(|_| subscribed && agy.is_none()).and_then(Auth::load);
        // Subscription endpoints are the providers' own: the API-key URL never carries a subscription token.
        let base = match (engine, auth.is_some()) {
            (Engine::Claude, true) => claude::API.to_owned(),
            (Engine::Gemini, true) => gemini_auth::CODE_ASSIST.to_owned(),
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
            agy,
        }
    }

    /// Can run Omega's tool loop (Antigravity answers in text only).
    pub fn tools(&self) -> bool {
        self.agy.is_none()
    }

    /// A Gemini API key speaks Google's OpenAI-compatible endpoint; only the subscription uses Code Assist.
    pub fn wire(&self) -> Wire {
        if self.agy.is_some() {
            return Wire::Antigravity;
        }
        match self.engine.wire() {
            Wire::Gemini if self.auth.is_none() => Wire::OpenAi,
            wire => wire,
        }
    }

    /// Same credential, another model of the same provider (Jev's tier, a Flash helper).
    pub fn with_model(&self, model: &str) -> Self {
        Self { model: model.trim().to_owned(), ..self.clone() }
    }

    /// Out of plan quota a moment ago: delegation skips it until the cool-down ends.
    pub fn cooling(&self) -> bool {
        let spent = spent().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        spent.get(&(self.engine, self.model.clone())).is_some_and(|at| at.elapsed() < COOLDOWN)
    }

    /// Remembers a quota failure or a refused account (other errors are transient and only fail over this once).
    pub fn note_failure(&self, error: &anyhow::Error) {
        let text = format!("{error:#}");
        let refused = [" 401", " 403", "#3501", "Code Assist", "non configuré"].iter().any(|m| text.contains(m));
        if exhausted(error) || refused {
            spent()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert((self.engine, self.model.clone()), std::time::Instant::now());
        }
    }

    /// Code Assist project: the stored one, else the environment, as the official CLI does.
    fn project(&self) -> String {
        let stored = self.auth.as_ref().map(|a| a.project()).unwrap_or_default();
        if stored.trim().is_empty() { gemini_auth::env_project() } else { stored }
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

    /// Changing the effort between requests keeps the prompt cache: DeepSeek, and Claude's adaptive thinking
    /// (a new thinking budget voids the cached messages).
    pub fn effort_keeps_cache(&self) -> bool {
        match self.wire() {
            Wire::OpenAi | Wire::Gemini | Wire::Antigravity => true,
            Wire::Anthropic => anthropic::adaptive(&self.model),
        }
    }

    /// A key is set (or a subscription is connected) and will only travel over TLS.
    pub fn configured(&self) -> bool {
        self.agy.is_some() || (self.base.starts_with("https://") && (self.auth.is_some() || !self.key.is_empty()))
    }

    pub fn subscription(&self) -> bool {
        self.auth.is_some() || self.agy.is_some()
    }

    pub fn account(&self) -> String {
        if self.agy.is_some() {
            return "Antigravity (agy)".to_owned();
        }
        self.auth.as_ref().map(|a| a.account()).unwrap_or_default()
    }

    /// Ahead of a turn: TLS connection opened and a near-expiry subscription token refreshed off the critical path.
    pub async fn warm(&self) {
        if !self.configured() || self.agy.is_some() {
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
            Wire::Gemini => gemini::body(messages, tools, &self.model, &self.project(), self.effort),
            // agy has no tool interface for Omega: only the conversation travels.
            Wire::Antigravity => json!({ "model": self.model, "messages": messages }),
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
            Wire::Antigravity => bail!("Antigravity ne compte pas les tokens sans exécuter une requête"),
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
            Wire::OpenAi | Wire::Gemini => {
                let mut body = body.clone();
                cap_output(&mut body, 1);
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
        if let Some(bin) = &self.agy {
            return Ok(agy::models(bin).await?.into_iter().map(|m| Choice::new(m.clone(), m)).collect());
        }
        anyhow::ensure!(self.base.starts_with("https://"), "URL {} non HTTPS refusée", self.engine.label());
        // Code Assist has no model list: the catalogue (and a free-form id) feeds the picker.
        if self.wire() == Wire::Gemini {
            return Ok(crate::domain::models::suggested(self.engine).iter().map(|m| Choice::new(m.id, m.name)).collect());
        }
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
        // Antigravity: installed and signed in when it lists its models.
        if let Some(bin) = &self.agy {
            let models = agy::models(bin).await?;
            anyhow::ensure!(!models.is_empty(), "agy ne liste aucun modèle — lancez `agy` une fois pour vous connecter");
            return Ok(());
        }
        // Code Assist: the account must have a tier and a project; the project found is kept for the requests.
        if self.wire() == Wire::Gemini {
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
        // The model's own request shape (thinking settings differ by generation), as small as it gets.
        let quiet = Self { effort: Effort::Off, ..self.clone() };
        let mut body = quiet.body(&[json!({ "role": "user", "content": "ok" })], None);
        cap_output(&mut body, 16);
        let res = self.send(&body).await?;
        let status = res.status();
        anyhow::ensure!(status.is_success(), "{} {status}", self.engine.label());
        Ok(())
    }

    pub async fn stream(&self, body: &Value, mut on: impl FnMut(Delta<'_>)) -> Result<Completion> {
        if let Some(bin) = &self.agy {
            let messages = body["messages"].as_array().cloned().unwrap_or_default();
            return agy::run(bin, &self.model, self.effort, &messages, on).await?.finish(self.engine);
        }
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
            Wire::Gemini => gemini::apply(out, data, on),
            Wire::Antigravity => bail!("agy ne passe pas par HTTP"),
        }
    }

    fn url(&self) -> String {
        match self.wire() {
            Wire::OpenAi => format!("{}/chat/completions", self.base),
            Wire::Anthropic => format!("{}/v1/messages", self.base),
            Wire::Gemini => format!("{}:streamGenerateContent?alt=sse", self.base),
            Wire::Antigravity => String::new(),
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
                Wire::Gemini => req.bearer_auth(&token).header("user-agent", gemini_auth::USER_AGENT),
                Wire::Antigravity => bail!("agy ne passe pas par HTTP"),
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
            // Plan quota spent: retrying only burns time, the caller falls back to another engine.
            if quota_error(code, &text) {
                bail!("{} {status}: {QUOTA} — {}", self.engine.label(), error_text(&text));
            }
            if !retryable(code, attempt) {
                if self.wire() == Wire::Gemini
                    && let Some(hint) = gemini_auth::hint(&text)
                {
                    bail!("{} {status}: {hint}", self.engine.label());
                }
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

const QUOTA: &str = "quota épuisé";
/// A model out of quota is not asked again before this (plans reset hourly to daily).
const COOLDOWN: Duration = Duration::from_secs(30 * 60);

/// (engine, model) → when its quota ran out, shared by every `Llm` handle of the process.
fn spent() -> &'static std::sync::Mutex<std::collections::HashMap<(Engine, String), std::time::Instant>> {
    static SPENT: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<(Engine, String), std::time::Instant>>> =
        std::sync::OnceLock::new();
    SPENT.get_or_init(Default::default)
}

/// A 429 that means "plan used up" rather than "too fast": no retry helps before the reset.
fn quota_error(status: u16, body: &str) -> bool {
    let lower = body.to_lowercase();
    status == 429
        && ["resource_exhausted", "quota", "exhausted your capacity", "usage limit"]
            .iter()
            .any(|m| lower.contains(m))
}

/// The engine ran out of plan quota (an error from a request): delegation moves on to the next engine.
pub fn exhausted(error: &anyhow::Error) -> bool {
    format!("{error:#}").contains(QUOTA)
}

/// Output cap of a probe or count request, in the wire's own field.
fn cap_output(body: &mut Value, tokens: u32) {
    if body.get("request").is_some() {
        body["request"]["generationConfig"]["maxOutputTokens"] = json!(tokens);
    } else {
        body["max_tokens"] = json!(tokens);
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

/// Empty or malformed tool arguments become `{}` so the tool can report a missing argument instead of crashing.
fn arguments(raw: &Value) -> Value {
    raw.as_str()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// `data:<media type>;base64,<data>` → `(media type, data)`.
fn data_url(url: &str) -> Option<(&str, &str)> {
    url.strip_prefix("data:")?.split_once(";base64,")
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
    fn a_gemini_key_speaks_the_openai_compatible_endpoint() {
        let s = Settings { gemini_auth: AuthMode::ApiKey, gemini_key: "AIza".into(), ..Settings::default() };
        let llm = Llm::new(crate::infra::http::client("").unwrap(), &s, Engine::Gemini);
        assert_eq!(llm.wire(), Wire::OpenAi);
        assert_eq!(llm.url(), "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions");
        let flash = llm.with_model(" gemini-3.8-flash ");
        assert_eq!(flash.body(&[json!({ "role": "user", "content": "x" })], None)["model"], "gemini-3.8-flash");
    }

    #[test]
    fn a_spent_quota_cools_the_model_down_but_other_errors_do_not() {
        let s = Settings { gemini_auth: AuthMode::ApiKey, gemini_key: "k".into(), ..Settings::default() };
        let llm = Llm::new(crate::infra::http::client("").unwrap(), &s, Engine::Gemini).with_model("gemini-cooldown-test");
        assert!(quota_error(429, r#"{"error":{"status":"RESOURCE_EXHAUSTED","message":"Quota exceeded"}}"#));
        assert!(!quota_error(429, r#"{"error":{"message":"Too many requests, slow down"}}"#), "trop rapide ≠ quota");
        llm.note_failure(&anyhow::anyhow!("Gemini injoignable"));
        assert!(!llm.cooling());
        llm.note_failure(&anyhow::anyhow!("Gemini 429: {QUOTA} — Quota exceeded"));
        assert!(llm.cooling());
        assert!(!llm.with_model("gemini-other").cooling(), "par modèle");
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
        let markup = Completion {
            content: "<｜DSML｜function_calls> <｜DSML｜invoke name=\"list_dir\"> </｜DSML｜invoke>".into(),
            ..Completion::default()
        };
        assert!(markup.finish(Engine::DeepSeek).is_err(), "balisage d'outil en texte : pas une réponse");
        assert!(Completion { content: "La balise DSML sert à…".into(), ..Completion::default() }.finish(Engine::DeepSeek).is_ok());
        assert!(thinking_rejected(&anyhow::anyhow!("Claude 400: messages.5.content.0: Invalid `signature` in `thinking` block")));
    }

    /// Gemini with the saved subscription, main and Flash models: `cargo test -- --ignored gemini_live --nocapture`.
    #[tokio::test]
    #[ignore = "network + Gemini subscription"]
    async fn gemini_live() {
        let (settings, _) = crate::infra::store::load_settings();
        let llm = Llm::new(crate::infra::http::client(&settings.proxy).unwrap(), &settings, Engine::Gemini);
        println!("subscription={} wire={:?} configured={}", llm.subscription(), llm.wire(), llm.configured());
        if let Some(auth) = llm.auth.clone().or_else(|| Auth::load(crate::domain::auth::Provider::Gemini)) {
            println!("project stocké: {:?} compte: {:?}", auth.project(), auth.account());
            let token = auth.fresh(&llm.http).await.unwrap();
            let probe = json!({ "metadata": { "pluginType": "GEMINI", "ideType": "IDE_UNSPECIFIED", "platform": "PLATFORM_UNSPECIFIED" } });
            let res = llm
                .http
                .post(format!("{}:loadCodeAssist", gemini_auth::CODE_ASSIST))
                .bearer_auth(&token)
                .header("user-agent", gemini_auth::USER_AGENT)
                .json(&probe)
                .send()
                .await
                .unwrap();
            println!("loadCodeAssist {}: {}", res.status(), res.text().await.unwrap_or_default());
            let quota = gemini_auth::post(&llm.http, &token, "retrieveUserQuota", &json!({ "project": auth.project() })).await;
            println!("retrieveUserQuota: {:?}", quota.map(|v| v.to_string().chars().take(1500).collect::<String>()).map_err(|e| format!("{e:#}")));
            let models = gemini_auth::post(&llm.http, &token, "fetchAvailableModels", &json!({})).await;
            println!("fetchAvailableModels: {:?}", models.map(|v| v.to_string().chars().take(3000).collect::<String>()).map_err(|e| format!("{e:#}")));
        }
        println!("ping: {:?}", llm.ping().await.map_err(|e| format!("{e:#}")));
        for model in [settings.gemini_flash_model.clone(), settings.gemini_model.clone()] {
            let m = llm.with_model(&model).with_effort(Effort::Off);
            let body = m.body(&[json!({ "role": "user", "content": "Reply with the single word: ok" })], None);
            match m.stream(&body, |_| {}).await {
                Ok(done) => println!("{model}: {:?} usage={:?}", done.content, done.usage),
                Err(e) => println!("{model}: ERREUR {e:#}"),
            }
        }
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
