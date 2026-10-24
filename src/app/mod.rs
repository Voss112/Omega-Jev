//! Use cases: routing (Jev), context assembly, agent loop (DeepSeek or Claude), health.

pub mod agent;
pub mod bench;
mod bench_output;
pub mod clear;
pub mod compact;
pub mod context;
pub mod dictation;
#[cfg(test)]
mod e2e;
pub mod health;
pub mod login;
pub mod prompts;
pub mod router;
pub mod savings;

use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, mpsc},
};

use anyhow::Result;
use reqwest::Client;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    domain::{
        auth::{Cred, Provider, Window},
        chat::{Message, Summary, Usage},
        engine::Engine,
        models::Choice,
        route::Route,
        savings::Sample,
        settings::Settings,
        tool::ToolKind,
    },
    infra::{http, jev::Jev, llm::Llm, paths, vpn::VpnInfo},
    tools::webindex::WebIndex,
};

/// Immutable runtime environment, rebuilt when settings change.
pub struct Env {
    pub settings: Settings,
    pub http: Client,
    pub llm: Llm,
    /// Flash models for delegated work (simple turns, tests, comments, summaries), in order of preference:
    /// Gemini Flash, then DeepSeek Flash. Empty when delegation is off.
    pub helpers: Vec<Llm>,
    pub jev: Option<Jev>,
    pub web: Arc<WebIndex>,
}

impl Env {
    pub fn new(settings: Settings, web: Arc<WebIndex>) -> Result<Self> {
        let http = http::client(&settings.proxy)?;
        let llm = Llm::new(http.clone(), &settings, settings.engine);
        let helpers = helpers(&http, &settings, &llm);
        let jev_key = if settings.jev_key.trim().is_empty() {
            std::env::var("TYPESAFE_API_KEY").unwrap_or_default()
        } else {
            settings.jev_key.trim().to_owned()
        };
        let jev = Jev::new(http.clone(), &settings.jev_url, &jev_key, &settings.jev_model);
        Ok(Self { settings, http, llm, helpers, jev, web })
    }

    /// Same environment, another engine (a delegated turn, or the effort Jev picked).
    pub fn with_llm(&self, llm: Llm) -> Self {
        Self {
            settings: self.settings.clone(),
            http: self.http.clone(),
            llm,
            helpers: Vec::new(),
            jev: self.jev.clone(),
            web: self.web.clone(),
        }
    }

    /// Engines that may write a conversation summary, cheapest first; the main engine closes the list.
    pub fn summarizers(&self) -> impl Iterator<Item = &Llm> {
        self.helpers.iter().filter(|h| !h.cooling()).chain(std::iter::once(&self.llm))
    }

    pub fn workspace(&self) -> PathBuf {
        self.settings.workspace.clone().filter(|p| p.is_dir()).unwrap_or_else(paths::workspace)
    }
}

/// Delegation chain: Gemini Flash (when preferred and connected), then DeepSeek Flash. A helper that is the
/// main engine itself is skipped: delegating to it would only change nothing.
fn helpers(http: &Client, settings: &Settings, main: &Llm) -> Vec<Llm> {
    if !settings.delegate {
        return Vec::new();
    }
    let gemini = settings
        .delegate_gemini
        .then(|| Llm::new(http.clone(), settings, Engine::Gemini).with_model(&settings.gemini_flash_model));
    let deepseek = Some(Llm::new(http.clone(), settings, Engine::DeepSeek));
    [gemini, deepseek]
        .into_iter()
        .flatten()
        .filter(|h| h.configured() && !h.model.is_empty() && !(h.engine == main.engine && h.model == main.model))
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ApprovalKind {
    /// `subject` is a shell command line.
    Command,
    /// `subject` is a unified diff.
    Edit,
}

pub struct Approval {
    pub conv: Uuid,
    pub kind: ApprovalKind,
    pub subject: String,
    pub reason: String,
    pub reply: oneshot::Sender<bool>,
}

pub enum Event {
    Routed {
        conv: Uuid,
        route: Route,
        preamble: String,
        /// Tools this turn surfaces mid-conversation (Claude deferred mode).
        tools_added: Vec<ToolKind>,
        toolset: BTreeSet<ToolKind>,
        /// Claude reasoning replay boundary and the prefix fingerprint it was checked against.
        thinking_from: usize,
        prefix: u64,
    },
    Delta {
        conv: Uuid,
        text: String,
        reasoning: bool,
    },
    Message {
        conv: Uuid,
        msg: Message,
    },
    Usage {
        conv: Uuid,
        usage: Usage,
    },
    /// One request measured for the economy ledger.
    Measured {
        conv: Uuid,
        sample: Sample,
        thinking_from: usize,
    },
    /// Prompt tokens of the latest request (drives auto-compaction).
    Context {
        conv: Uuid,
        tokens: u64,
    },
    Approval(Approval),
    /// Files were changed during the turn keyed `turn`: undo is available.
    Checkpointed {
        conv: Uuid,
        turn: i64,
    },
    Compacted {
        conv: Uuid,
        summary: Summary,
    },
    /// Tool results (message indices) replaced on the wire by stubs mid-turn.
    Cleared {
        conv: Uuid,
        indices: Vec<usize>,
    },
    Done {
        conv: Uuid,
        error: Option<String>,
    },
    /// The Antigravity CLI finished installing (path) or failed; `login` opens its sign-in console next.
    AgyInstalled {
        result: Result<String, String>,
        login: bool,
    },
    /// GitHub device flow: the code to type on the verification page.
    DeviceCode {
        code: String,
        url: String,
    },
    /// A subscription finished (or failed) its OAuth dance.
    Connected {
        provider: Provider,
        result: Result<Cred, String>,
    },
    /// Best-effort plan usage of a subscription.
    PlanUsage {
        provider: Provider,
        windows: Vec<Window>,
    },
    /// Model ids the account may call, for the pickers.
    Models {
        engine: Engine,
        result: Result<Vec<Choice>, String>,
    },
    Health(Vec<health::Health>),
    /// Live token-economy benchmark results.
    Bench(Vec<health::Health>),
    Vpn(VpnInfo),
}

/// Sends events to the UI and wakes it.
#[derive(Clone)]
pub struct Emitter {
    tx: mpsc::Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Emitter {
    pub fn new(tx: mpsc::Sender<Event>, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { tx, wake }
    }

    pub fn send(&self, event: Event) {
        // Streaming deltas ride the UI's 30 FPS busy cadence: waking per token would redraw at the token rate.
        let wake = !matches!(event, Event::Delta { .. });
        if self.tx.send(event).is_ok() && wake {
            (self.wake)();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn deltas_do_not_wake_the_ui_but_other_events_do() {
        let (tx, rx) = mpsc::channel();
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        let emit = Emitter::new(tx, Arc::new(move || _ = counter.fetch_add(1, Ordering::SeqCst)));
        emit.send(Event::Delta { conv: Uuid::nil(), text: "a".into(), reasoning: false });
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
        emit.send(Event::Done { conv: Uuid::nil(), error: None });
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert_eq!(rx.try_iter().count(), 2);
    }
}
