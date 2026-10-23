//! One user turn: compaction → Jev routes → Kimi streams → tools run → loop until a final answer.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Result, bail, ensure};
use futures_util::future::join_all;

use super::{Emitter, Env, Event, compact, context, router};
use crate::{
    domain::{
        chat::{Conversation, Message, ToolCall, Usage},
        tool::ToolKind,
    },
    infra::llm::Delta,
    tools::{self, ToolCtx, ToolOutput, checkpoint::Checkpoint, diagnostics},
};

pub async fn run(mut conv: Conversation, env: Arc<Env>, emit: Emitter) {
    let id = conv.id;
    let turn = conv.messages.last().map_or(0, Message::turn_key);
    let error = turn_loop(&mut conv, &env, &emit, turn).await.err().map(|e| format!("{e:#}"));
    if Checkpoint::new(id, turn).exists() {
        emit.send(Event::Checkpointed { conv: id, turn });
    }
    emit.send(Event::Done { conv: id, error });
}

async fn turn_loop(conv: &mut Conversation, env: &Env, emit: &Emitter, turn: i64) -> Result<()> {
    ensure!(env.llm.configured(), "Clé API {} manquante — ouvrez ⚙ Paramètres.", env.llm.engine.label());
    // Compaction is an optimization: on failure the turn simply runs on the full history.
    let _ = compact::maybe(conv, env, emit).await;
    let root = env.workspace();
    let query = conv.messages.last().map(|m| m.content.clone()).unwrap_or_default();

    let (route, jev_tokens) = router::route(env, conv, &query).await;
    let loaded: Vec<&str> = conv.messages.iter().rev().skip(1).filter_map(|m| m.route.as_ref()?.playbook.as_deref()).collect();
    let preamble = context::preamble(&route, &root, conv.user_turns() == 1, &loaded);
    conv.toolset.extend(route.tools());
    if let Some(m) = conv.last_user_mut() {
        m.preamble.clone_from(&preamble);
        m.route = Some(route.clone());
    }
    emit.send(Event::Routed { conv: conv.id, route: route.clone(), preamble, toolset: conv.toolset.clone() });
    if jev_tokens > 0 {
        emit.send(Event::Usage { conv: conv.id, usage: Usage::jev(jev_tokens) });
    }

    let ctx = ToolCtx { env, root, emit, conv: conv.id, turn };
    for _ in 0..env.settings.max_steps.max(1) {
        let body = context::request(conv, env);
        let id = conv.id;
        let done = env
            .llm
            .stream(&body, |delta| {
                let (text, reasoning) = match delta {
                    Delta::Text(t) => (t, false),
                    Delta::Reasoning(t) => (t, true),
                };
                emit.send(Event::Delta { conv: id, text: text.to_owned(), reasoning });
            })
            .await?;
        // Prompt size drives compaction; estimated (~4 bytes/token) if the server sent no usage.
        conv.context = if done.usage.prompt > 0 { done.usage.prompt } else { body.to_string().len() as u64 / 4 };
        emit.send(Event::Usage { conv: id, usage: done.usage });
        emit.send(Event::Context { conv: id, tokens: conv.context });

        let calls = done.tool_calls.clone();
        commit(conv, emit, Message::assistant(done.content, done.reasoning, done.tool_calls));
        if calls.is_empty() {
            return Ok(());
        }
        run_tools(conv, &ctx, &calls).await;
    }
    bail!("Limite de {} étapes atteinte — envoyez « continue » pour poursuivre.", env.settings.max_steps)
}

/// Read-only calls run concurrently; writes and shell stay sequential and ordered.
/// The last output of the step carries the diagnostics of any file change.
async fn run_tools(conv: &mut Conversation, ctx: &ToolCtx<'_>, calls: &[ToolCall]) {
    let mut changed: Vec<PathBuf> = Vec::new();
    let mut held: Option<(&ToolCall, ToolOutput)> = None;
    for group in groups(calls) {
        let outputs = join_all(group.iter().map(|c| tools::run(ctx, c))).await;
        for (call, output) in group.iter().zip(outputs) {
            if let Some((c, o)) = held.take() {
                commit_tool(conv, ctx.emit, c, o);
            }
            changed.extend(output.changed.clone());
            held = Some((call, output));
        }
    }
    let Some((call, mut output)) = held else { return };
    if ctx.env.settings.auto_diagnostics
        && !changed.is_empty()
        && let Some(report) = diagnostics::check(&ctx.root, &changed).await
    {
        output.text.push_str("\n\n");
        output.text.push_str(&report);
    }
    commit_tool(conv, ctx.emit, call, output);
}

fn groups(calls: &[ToolCall]) -> Vec<&[ToolCall]> {
    let read_only = |c: &ToolCall| ToolKind::parse(&c.name).is_some_and(ToolKind::read_only);
    let mut out = Vec::new();
    let mut start = 0;
    while start < calls.len() {
        let run = calls[start..].iter().take_while(|c| read_only(c)).count().max(1);
        out.push(&calls[start..start + run]);
        start += run;
    }
    out
}

fn commit_tool(conv: &mut Conversation, emit: &Emitter, call: &ToolCall, output: ToolOutput) {
    let msg = Message { diff: output.diff, ..Message::tool(call.id.clone(), output.text) };
    commit(conv, emit, msg);
}

fn commit(conv: &mut Conversation, emit: &Emitter, msg: Message) {
    conv.push(msg.clone());
    emit.send(Event::Message { conv: conv.id, msg });
}
