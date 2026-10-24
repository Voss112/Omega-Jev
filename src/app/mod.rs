//! Use cases: routing (Jev), context assembly, agent loop (DeepSeek or Claude), health.

pub mod agent;
pub mod bench;
pub mod compact;
pub mod context;
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
    pub jev: Option<Jev>,
    pub web: Arc<WebIndex>,
}

impl Env {
    pub fn new(settings: Settings, web: Arc<WebIndex>) -> Result<Self> {
        let http = http::client(&settings.proxy)?;
        let llm = Llm::new(http.clone(), &settings, settings.engine);
        let jev_key = if settings.jev_key.trim().is_empty() {
            std::env::var("TYPESAFE_API_KEY").unwrap_or_default()
        } else {
            settings.jev_key.trim().to_owned()
        };
        let jev = Jev::new(http.clone(), &settings.jev_url, &jev_key, &settings.jev_model);
        Ok(Self { settings, http, llm, jev, web })
    }

    pub fn workspace(&self) -> PathBuf {
        self.settings.workspace.clone().filter(|p| p.is_dir()).unwrap_or_else(paths::workspace)
    }
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
    Done {
        conv: Uuid,
        error: Option<String>,
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
        if self.tx.send(event).is_ok() {
            (self.wake)();
        }
    }
}
