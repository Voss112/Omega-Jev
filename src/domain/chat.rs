use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{engine::Engine, route::Route, savings::Ledger, tool::ToolKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// One block of a Claude answer, in stream order. A tool loop must replay them unchanged:
/// signed reasoning is checked against the blocks around it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    /// Reasoning (empty when the API omits its text) and the signature that carries it.
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    Text {
        text: String,
    },
    /// Index into the message's `tool_calls`.
    ToolUse {
        call: usize,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Image {
    /// `image/png`, `image/jpeg`…
    pub media: String,
    /// Base64 of the original bytes (resized client-side before storing).
    pub data: String,
    /// Content hash, computed once: hashing megabytes of base64 on every frame costs milliseconds.
    #[serde(skip)]
    key: std::sync::OnceLock<u64>,
}

impl Image {
    pub fn new(media: impl Into<String>, data: String) -> Self {
        Self { media: media.into(), data, key: std::sync::OnceLock::new() }
    }

    /// Stable content hash (texture cache key).
    pub fn key(&self) -> u64 {
        *self.key.get_or_init(|| {
            use std::hash::{DefaultHasher, Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            self.media.hash(&mut hasher);
            self.data.hash(&mut hasher);
            hasher.finish()
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Screenshots shown to the model (only vision engines receive them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
    /// Hidden orchestrator context sent before `content`; stored so the wire prefix stays cache-stable.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preamble: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Claude answers: exact block layout, signed reasoning included.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
    /// Tool messages: unified diff of the file change (shown in the UI, never sent to the model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// Tool messages: bytes the RTK compressor removed from the raw output (economy ledger).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rtk: u64,
    /// User messages: files changed during this turn can be restored.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub checkpoint: bool,
    /// Engine and exact model id that produced this assistant message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<Engine>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    pub at: DateTime<Utc>,
}

impl Message {
    fn new(role: Role, content: String) -> Self {
        Self {
            role,
            content,
            images: Vec::new(),
            preamble: String::new(),
            reasoning: String::new(),
            tool_calls: Vec::new(),
            parts: Vec::new(),
            tool_call_id: None,
            route: None,
            diff: None,
            rtk: 0,
            checkpoint: false,
            engine: None,
            model: String::new(),
            at: Utc::now(),
        }
    }

    pub fn user(content: String) -> Self {
        Self::new(Role::User, content)
    }

    /// User message carrying screenshots; text may be empty.
    pub fn user_with(content: String, images: Vec<Image>) -> Self {
        Self { images, ..Self::new(Role::User, content) }
    }

    pub fn assistant(content: String, reasoning: String, tool_calls: Vec<ToolCall>) -> Self {
        Self { reasoning, tool_calls, ..Self::new(Role::Assistant, content) }
    }

    /// Stamps the model that actually answered, so history stays readable after a switch.
    pub fn by(mut self, engine: Engine, model: &str) -> Self {
        self.engine = Some(engine);
        self.model = model.trim().to_owned();
        self
    }

    pub fn tool(call_id: String, content: String) -> Self {
        Self { tool_call_id: Some(call_id), ..Self::new(Role::Tool, content) }
    }

    /// Stable id of the turn started by this (user) message; names its checkpoint.
    pub fn turn_key(&self) -> i64 {
        self.at.timestamp_millis()
    }
}

/// Model-written digest replacing `messages[..upto]` on the wire (UI keeps the full history).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub upto: usize,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt: u64,
    pub completion: u64,
    pub cached: u64,
    pub reasoning: u64,
    pub jev: u64,
    /// Claude: prompt tokens written to the cache (billed at the write premium).
    #[serde(default)]
    pub written: u64,
}

impl Usage {
    pub fn jev(tokens: u64) -> Self {
        Self { jev: tokens, ..Self::default() }
    }

    pub fn cache_ratio(&self) -> f32 {
        if self.prompt == 0 { 0.0 } else { self.cached as f32 / self.prompt as f32 }
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, o: Self) {
        self.prompt += o.prompt;
        self.completion += o.completion;
        self.cached += o.cached;
        self.reasoning += o.reasoning;
        self.jev += o.jev;
        self.written += o.written;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conversation {
    pub id: Uuid,
    pub title: String,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    /// Monotonic per conversation: tools once loaded stay loaded, keeping the request prefix cache-stable.
    #[serde(default)]
    pub toolset: BTreeSet<ToolKind>,
    #[serde(default)]
    pub usage: Usage,
    /// Prompt size of the latest request (drives auto-compaction).
    #[serde(default)]
    pub context: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    /// Measured token economy of this conversation.
    #[serde(default)]
    pub savings: Ledger,
    /// Claude reasoning is replayed from this message on: earlier blocks were signed against another
    /// system prompt, tool set or (compacted) history, and only a leading run of them may be dropped.
    #[serde(default)]
    pub thinking_from: usize,
    /// Fingerprint of the system prompt and tool set the replayed reasoning was signed against.
    #[serde(default)]
    pub prefix: u64,
    pub messages: Vec<Message>,
}

impl Default for Conversation {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            title: "Nouvelle discussion".into(),
            created: now,
            updated: now,
            toolset: BTreeSet::new(),
            usage: Usage::default(),
            context: 0,
            summary: None,
            savings: Ledger::default(),
            thinking_from: 0,
            prefix: 0,
            messages: Vec::new(),
        }
    }
}

impl Conversation {
    pub fn push(&mut self, msg: Message) {
        if self.messages.is_empty() && msg.role == Role::User {
            self.title = title_of(if msg.content.trim().is_empty() { "Image" } else { &msg.content });
        }
        self.updated = Utc::now();
        self.messages.push(msg);
    }

    pub fn last_user_mut(&mut self) -> Option<&mut Message> {
        self.messages.iter_mut().rev().find(|m| m.role == Role::User)
    }

    pub fn user_turns(&self) -> usize {
        self.messages.iter().filter(|m| m.role == Role::User).count()
    }

    /// Drops `messages[from..]` (edit / regenerate) and any summary covering them.
    pub fn truncate(&mut self, from: usize) {
        self.messages.truncate(from);
        if self.summary.as_ref().is_some_and(|s| s.upto > from) {
            self.summary = None;
        }
        self.context = 0;
        self.thinking_from = self.thinking_from.min(from);
    }

    /// Answers every dangling tool call (interrupted run) so the API accepts the history.
    /// A call is answered only by the results that follow its own message: ids repeat across steps (older OpenAI-compatible engines).
    pub fn repair(&mut self) {
        let interrupted = |id: String| Message::tool(id, "ERROR: interrupted by user".into());
        let mut fixed = Vec::with_capacity(self.messages.len());
        let mut pending: Vec<String> = Vec::new();
        for msg in self.messages.drain(..) {
            if msg.role == Role::Tool {
                if let Some(i) = msg.tool_call_id.as_ref().and_then(|id| pending.iter().position(|p| p == id)) {
                    pending.remove(i);
                }
                fixed.push(msg);
                continue;
            }
            fixed.extend(pending.drain(..).map(interrupted));
            pending = msg.tool_calls.iter().map(|c| c.id.clone()).collect();
            fixed.push(msg);
        }
        fixed.extend(pending.into_iter().map(interrupted));
        self.messages = fixed;
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn title_of(text: &str) -> String {
    const MAX: usize = 48;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Discussion");
    let mut title: String = line.chars().take(MAX).collect();
    if line.chars().count() > MAX {
        title.push('…');
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_answers_dangling_tool_calls() {
        let mut conv = Conversation::default();
        conv.push(Message::user("hi".into()));
        let call = ToolCall { id: "c1".into(), name: "grep".into(), arguments: "{}".into() };
        conv.push(Message::assistant(String::new(), String::new(), vec![call]));
        conv.repair();
        assert_eq!(conv.messages.last().and_then(|m| m.tool_call_id.as_deref()), Some("c1"));
        assert_eq!(conv.title, "hi");
    }

    #[test]
    fn an_id_answered_in_an_earlier_step_does_not_hide_a_dangling_call() {
        let call = || ToolCall { id: "functions.grep:0".into(), name: "grep".into(), ..ToolCall::default() };
        let mut conv = Conversation::default();
        conv.push(Message::user("hi".into()));
        conv.push(Message::assistant(String::new(), String::new(), vec![call()]));
        conv.push(Message::tool("functions.grep:0".into(), "1 hit".into()));
        conv.push(Message::assistant(String::new(), String::new(), vec![call()]));
        conv.repair();
        assert_eq!(conv.messages.len(), 5, "le second appel reçoit sa propre réponse");
        assert_eq!(conv.messages[4].content, "ERROR: interrupted by user");
        conv.repair();
        assert_eq!(conv.messages.len(), 5, "réparer deux fois ne duplique rien");
    }
}
