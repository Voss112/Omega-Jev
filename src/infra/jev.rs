//! Jev (TypeSafe System One) client: typed decisions, zero prose. POST /v1/systemone.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::{Map, Value, json};

pub enum Question {
    Choice {
        instructions: String,
        criteria: Vec<(String, String)>,
    },
    /// Levels ordered lowest → highest.
    Score {
        instructions: String,
        levels: Vec<String>,
    },
    Noul {
        instructions: String,
    },
}

impl Question {
    pub fn noul(instructions: &str) -> Self {
        Self::Noul { instructions: instructions.into() }
    }

    fn wire(&self) -> Value {
        match self {
            Self::Choice { instructions, criteria } => {
                let map: Map<String, Value> = criteria.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
                json!({ "type": "choice", "instructions": instructions, "criteria": map })
            }
            Self::Score { instructions, levels } => json!({ "type": "score", "instructions": instructions, "criteria": levels }),
            Self::Noul { instructions } => json!({ "type": "noul", "instructions": instructions }),
        }
    }
}

pub struct Answers {
    map: Map<String, Value>,
    pub tokens: u64,
}

impl Answers {
    /// (chosen option, confidence)
    pub fn choice(&self, id: &str) -> Option<(String, f32)> {
        let a = self.map.get(id)?;
        Some((a["choice"].as_str()?.to_owned(), a["confidence"].as_f64().unwrap_or(0.0) as f32))
    }

    /// Probability in [0, 1].
    pub fn noul(&self, id: &str) -> Option<f32> {
        self.map.get(id)?["noul"].as_f64().map(|v| v as f32)
    }

    /// Expected level normalized to [0, 1], computed from the level probabilities.
    pub fn score(&self, id: &str) -> Option<f32> {
        let probs = self.map.get(id)?["probabilities"].as_object()?;
        let top = probs.keys().filter_map(|k| k.parse::<f32>().ok()).fold(0.0_f32, f32::max);
        if top == 0.0 {
            return None;
        }
        let sum: f32 = probs.iter().filter_map(|(k, p)| Some(k.parse::<f32>().ok()? * p.as_f64()? as f32)).sum();
        Some(sum / top)
    }
}

#[derive(Clone)]
pub struct Jev {
    http: Client,
    url: String,
    key: String,
    model: String,
}

/// Jev sits on the critical path of every turn and shell command: past this, local heuristics answer instead.
const TIMEOUT: Duration = Duration::from_secs(6);

impl Jev {
    /// `None` without a key or without TLS (the key must never travel in clear).
    pub fn new(http: Client, url: &str, key: &str, model: &str) -> Option<Self> {
        (!key.is_empty() && url.starts_with("https://")).then(|| Self {
            http,
            url: url.into(),
            key: key.into(),
            model: model.into(),
        })
    }

    /// Opens the TLS connection ahead of the first question (no key sent).
    pub async fn warm(&self) {
        crate::infra::http::warm(&self.http, &self.url).await;
    }

    /// All questions are answered in parallel by one call.
    pub async fn ask(&self, state: Value, questions: &[(String, Question)]) -> Result<Answers> {
        let questions: Map<String, Value> = questions.iter().map(|(id, q)| (id.clone(), q.wire())).collect();
        let body = json!({ "model": self.model, "state": state, "questions": questions });
        let res = self
            .http
            .post(&self.url)
            .bearer_auth(&self.key)
            .timeout(TIMEOUT)
            .json(&body)
            .send()
            .await
            .context("Jev injoignable")?;
        let status = res.status();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let msg = ["/error/message", "/detail/message", "/error", "/detail"]
                .iter()
                .find_map(|p| v.pointer(p).and_then(Value::as_str))
                .unwrap_or("");
            bail!("Jev {status}: {msg}");
        }
        let map = v["answers"].as_object().cloned().context("réponse Jev sans `answers`")?;
        let tokens = v["usage"]["input_tokens"].as_u64().unwrap_or(0) + v["usage"]["output_tokens"].as_u64().unwrap_or(0);
        Ok(Answers { map, tokens })
    }
}
