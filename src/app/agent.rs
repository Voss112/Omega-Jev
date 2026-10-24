//! One user turn: compaction → Jev routes → the engine streams → tools run → loop until a final answer.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Result, bail, ensure};
use futures_util::future::join_all;
use uuid::Uuid;

use super::{Emitter, Env, Event, compact, context, prompts, router, savings};
use crate::{
    domain::{
        chat::{Conversation, Message, ToolCall, Usage},
        tool::ToolKind,
    },
    infra::llm::{self, Delta},
    tools::{self, ToolCtx, ToolOutput, checkpoint::Checkpoint, diagnostics, reads::Reads},
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
    ensure!(
        env.llm.configured(),
        "{} non configuré — connectez l'abonnement ou saisissez une clé dans ⚙ Paramètres.",
        env.llm.engine.label()
    );
    // Routing reads a snapshot, so it runs while compaction (an optimization: on failure the turn simply runs
    // on the full history) rewrites the summary.
    let input = router::Input::of(conv);
    let (_, (route, jev_tokens)) = tokio::join!(compact::maybe(conv, env, emit), router::route(env, &input));
    let root = env.workspace();
    // A playbook summarized away by compaction is no longer on the wire: it must be sent again in full.
    let visible = conv.summary.as_ref().map_or(0, |s| s.upto).min(conv.messages.len());
    let loaded: Vec<&str> =
        conv.messages[visible..].iter().rev().skip(1).filter_map(|m| m.route.as_ref()?.playbook.as_deref()).collect();
    let preamble = context::preamble(&route, &root, conv.user_turns() == 1, &loaded);
    conv.toolset.extend(route.tools());
    if let Some(m) = conv.last_user_mut() {
        m.preamble.clone_from(&preamble);
        m.route = Some(route.clone());
    }
    // Frozen for the whole turn: the tool loop then only appends, which Claude's signed reasoning requires.
    let system = prompts::system(&env.settings, &root);
    // A new system prompt or tool set voids the reasoning signed before it: drop it as a leading run, once.
    let prefix = savings::prefix(&system, &conv.toolset);
    if prefix != conv.prefix {
        conv.prefix = prefix;
        conv.thinking_from = conv.messages.len().saturating_sub(1);
    }
    emit.send(Event::Routed {
        conv: conv.id,
        route: route.clone(),
        preamble,
        toolset: conv.toolset.clone(),
        thinking_from: conv.thinking_from,
        prefix,
    });
    if jev_tokens > 0 {
        emit.send(Event::Usage { conv: conv.id, usage: Usage::jev(jev_tokens) });
    }

    let reads = Reads::from_history(&conv.messages[visible..]);
    let ctx = ToolCtx { env, root, emit, conv: conv.id, turn, reads: &reads };
    for _ in 0..env.settings.max_steps.max(1) {
        let body = context::request(conv, env, &system, true);
        let done = match complete(env, emit, conv.id, &body).await {
            // Reasoning bound to an older state of the conversation: one retry without the reasoning blocks.
            Err(e) if llm::thinking_rejected(&e) => {
                // Later steps of this turn keep only the reasoning they produce themselves.
                conv.thinking_from = conv.messages.len();
                complete(env, emit, conv.id, &context::request(conv, env, &system, false)).await?
            }
            done => done?,
        };
        let sample = savings::measure(conv, env, &body, &done.usage);
        conv.savings.record(&sample);
        emit.send(Event::Measured { conv: conv.id, sample, thinking_from: conv.thinking_from });
        // Prompt size drives compaction; estimated (~4 bytes/token) if the server sent no usage.
        conv.context = if done.usage.prompt > 0 { done.usage.prompt } else { body.to_string().len() as u64 / 4 };
        emit.send(Event::Usage { conv: conv.id, usage: done.usage });
        emit.send(Event::Context { conv: conv.id, tokens: conv.context });

        let calls = done.tool_calls.clone();
        let answer = Message { parts: done.parts, ..Message::assistant(done.content, done.reasoning, done.tool_calls) };
        commit(conv, emit, answer.by(env.llm.engine, &env.llm.model));
        if calls.is_empty() {
            return Ok(());
        }
        run_tools(conv, &ctx, &calls).await;
    }
    bail!("Limite de {} étapes atteinte — envoyez « continue » pour poursuivre.", env.settings.max_steps)
}

/// One streamed model call, deltas forwarded to the UI.
async fn complete(env: &Env, emit: &Emitter, conv: Uuid, body: &serde_json::Value) -> Result<llm::Completion> {
    env.llm
        .stream(body, |delta| {
            let (text, reasoning) = match delta {
                Delta::Text(t) => (t, false),
                Delta::Reasoning(t) => (t, true),
            };
            emit.send(Event::Delta { conv, text: text.to_owned(), reasoning });
        })
        .await
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
    let msg = Message { diff: output.diff, rtk: output.rtk, ..Message::tool(call.id.clone(), output.text) };
    commit(conv, emit, msg);
}

fn commit(conv: &mut Conversation, emit: &Emitter, msg: Message) {
    conv.push(msg.clone());
    emit.send(Event::Message { conv: conv.id, msg });
}
