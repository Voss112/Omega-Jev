use std::collections::{BTreeSet, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{route::Route, tool::ToolKind};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Hidden orchestrator context sent before `content`; stored so the wire prefix stays cache-stable.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preamble: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
    /// Tool messages: unified diff of the file change (shown in the UI, never sent to the model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// User messages: files changed during this turn can be restored.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub checkpoint: bool,
    pub at: DateTime<Utc>,
}

impl Message {
    fn new(role: Role, content: String) -> Self {
        Self {
            role,
            content,
            preamble: String::new(),
            reasoning: String::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            route: None,
            diff: None,
            checkpoint: false,
            at: Utc::now(),
        }
    }

    pub fn user(content: String) -> Self {
        Self::new(Role::User, content)
    }

    pub fn assistant(content: String, reasoning: String, tool_calls: Vec<ToolCall>) -> Self {
        Self { reasoning, tool_calls, ..Self::new(Role::Assistant, content) }
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
            messages: Vec::new(),
        }
    }
}

impl Conversation {
    pub fn push(&mut self, msg: Message) {
        if self.messages.is_empty() && msg.role == Role::User {
            self.title = title_of(&msg.content);
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
    }

    /// Answers every dangling tool call (interrupted run) so the API accepts the history.
    pub fn repair(&mut self) {
        let answered: HashSet<String> = self.messages.iter().filter_map(|m| m.tool_call_id.clone()).collect();
        let mut fixed = Vec::with_capacity(self.messages.len());
        for msg in self.messages.drain(..) {
            let missing: Vec<String> =
                msg.tool_calls.iter().filter(|c| !answered.contains(&c.id)).map(|c| c.id.clone()).collect();
            fixed.push(msg);
            fixed.extend(missing.into_iter().map(|id| Message::tool(id, "ERROR: interrupted by user".into())));
        }
        self.messages = fixed;
    }
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
}
