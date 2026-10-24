//! Auto-compaction: past `compact_at` prompt tokens, older turns are summarized once and
//! replaced on the wire by that summary. The UI keeps the full history.

use std::fmt::Write as _;

use anyhow::Result;
use serde_json::json;

use super::{Emitter, Env, Event, savings};
use crate::{
    domain::{
        chat::{Conversation, Role, Summary},
        engine::Effort,
    },
    tools::compress::clip,
};

/// Turns kept verbatim after compaction (the current one included).
const KEEP_TURNS: usize = 2;
const TRANSCRIPT_MAX: usize = 400_000;

const PROMPT: &str = "Summarize this conversation so an engineer can continue it without the transcript. \
Keep: goals, decisions and their reasons, files/symbols touched, commands run and results, current state, open tasks, \
user preferences. Exact names, paths, numbers, errors. Terse bullets, no filler. Same language as the user.";

pub async fn maybe(conv: &mut Conversation, env: &Env, emit: &Emitter) -> Result<()> {
    if conv.context < env.settings.compact_at {
        return Ok(());
    }
    let users: Vec<usize> = conv.messages.iter().enumerate().filter(|(_, m)| m.role == Role::User).map(|(i, _)| i).collect();
    let Some(&upto) = users.len().checked_sub(KEEP_TURNS).and_then(|i| users.get(i)) else { return Ok(()) };
    let from = conv.summary.as_ref().map_or(0, |s| s.upto);
    if upto <= from {
        return Ok(());
    }
    let messages =
        vec![json!({ "role": "system", "content": PROMPT }), json!({ "role": "user", "content": transcript(conv, from, upto) })];
    // Summarizing needs no reasoning: thinking off (or the shortest a model allows) on whichever engine writes it.
    // A Flash model out of quota or unreachable hands over to the next one, the main engine last.
    let mut written = None;
    let mut last_error = None;
    for engine in env.summarizers() {
        let llm = engine.with_effort(Effort::Off);
        let body = llm.body(&messages, None);
        match llm.stream(&body, |_| {}).await {
            Ok(done) => {
                written = Some((llm, body, done));
                break;
            }
            Err(e) => {
                engine.note_failure(&e);
                last_error = Some(e);
            }
        }
    }
    let Some((llm, body, done)) = written else {
        return Err(last_error.unwrap_or_else(|| anyhow::anyhow!("aucun moteur pour résumer")));
    };
    let llm = &llm;
    emit.send(Event::Usage { conv: conv.id, usage: done.usage });
    let sample = savings::plain(llm, &body, &done.usage);
    conv.savings.record(&sample);
    emit.send(Event::Measured { conv: conv.id, sample, thinking_from: conv.thinking_from });
    // An empty summary would silently erase the history: keep the full transcript instead.
    anyhow::ensure!(!done.content.trim().is_empty(), "empty summary");
    let summary = Summary { upto, text: done.content };
    conv.summary = Some(summary.clone());
    conv.context = 0;
    // Kept answers were signed against the full history: their reasoning cannot follow the summary.
    conv.thinking_from = conv.messages.len().saturating_sub(1);
    emit.send(Event::Compacted { conv: conv.id, summary });
    Ok(())
}

fn transcript(conv: &Conversation, from: usize, upto: usize) -> String {
    let mut out = conv.summary.as_ref().map(|s| format!("PREVIOUS SUMMARY:\n{}\n\n", s.text)).unwrap_or_default();
    for m in &conv.messages[from..upto] {
        let _ = match m.role {
            Role::User => writeln!(out, "USER: {}", m.content),
            Role::Assistant => {
                let calls: Vec<String> =
                    m.tool_calls.iter().map(|c| format!("{}({})", c.name, clip(&c.arguments, 160))).collect();
                writeln!(
                    out,
                    "ASSISTANT: {}{}",
                    m.content,
                    if calls.is_empty() { String::new() } else { format!(" [calls: {}]", calls.join("; ")) }
                )
            }
            Role::Tool => writeln!(out, "TOOL RESULT: {}", clip(&m.content, 300)),
        };
    }
    clip(&out, TRANSCRIPT_MAX)
}
