//! One user turn: compaction → Jev routes → the engine streams → tools run → loop until a final answer.
//! Who answers: a Flash helper for delegable work (Gemini Flash, then DeepSeek Flash), otherwise the main engine,
//! on Claude at the tier Jev's difficulty calls for (escalating to the chosen model if the cheaper one stalls).

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Result, bail, ensure};
use futures_util::future::join_all;
use uuid::Uuid;

use super::{Emitter, Env, Event, clear, compact, context, prompts, router, savings};
use crate::{
    domain::{
        chat::{Conversation, Message, ToolCall, Usage},
        engine::Engine,
        route::{Route, Source},
        settings::Settings,
        tier,
        tool::ToolKind,
    },
    infra::llm::{self, Delta, Llm},
    tools::{
        self, ToolCtx, ToolOutput,
        checkpoint::Checkpoint,
        diagnostics,
        livetest::{self, Runner},
        reads::Reads,
    },
};

/// Steps a cheaper Claude tier gets before the chosen model takes the turn over.
const TIER_STEPS: u32 = 24;
/// How long the end of a turn waits for the live run its last edit started.
const LIVE_WAIT: Duration = Duration::from_secs(60);
/// User turns an app tool (CLI-Anything, kits, audit kit) stays loaded without being called.
const APP_IDLE_TURNS: usize = 3;

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
    // Tests start (or keep) running in the background while this turn codes.
    let runner = if env.settings.live_tests && (route.needs_code() || route.routine_task()) {
        livetest::ensure(&root).await
    } else {
        None
    };
    let mut live = LiveTurn::new(runner);
    // A playbook summarized away by compaction is no longer on the wire: it must be sent again in full.
    let visible = conv.summary.as_ref().map_or(0, |s| s.upto).min(conv.messages.len());
    let loaded: Vec<&str> =
        conv.messages[visible..].iter().rev().skip(1).filter_map(|m| m.route.as_ref()?.playbook.as_deref()).collect();
    let mut preamble = context::preamble(&route, &root, conv.user_turns() == 1, &loaded);
    if let Some(note) = live.failing_now() {
        preamble.push('\n');
        preamble.push_str(&note);
    }
    // App tools nobody called for a few turns leave the request (the CLI-Anything app, kits, audit kit).
    prune_idle_apps(conv, &route);
    conv.toolset.extend(route.tools());
    let tools_added = if env.llm.deferred_tools() { context::tool_additions(conv, visible) } else { Vec::new() };
    if let Some(m) = conv.last_user_mut() {
        m.preamble.clone_from(&preamble);
        m.route = Some(route.clone());
        m.tools_added.clone_from(&tools_added);
    }
    // Frozen for the whole turn: the tool loop then only appends, which Claude's signed reasoning requires.
    let system = prompts::system(&env.settings, &root);
    // A new system prompt or tool set voids the reasoning signed before it: drop it as a leading run, once.
    // Deferred mode ships every tool on every request, so only the system prompt matters there.
    let prefix = wire_prefix(env, &system, conv);
    if prefix != conv.prefix {
        conv.prefix = prefix;
        conv.thinking_from = conv.messages.len().saturating_sub(1);
    }
    emit.send(Event::Routed {
        conv: conv.id,
        route: route.clone(),
        preamble,
        tools_added,
        toolset: conv.toolset.clone(),
        thinking_from: conv.thinking_from,
        prefix,
    });
    if jev_tokens > 0 {
        emit.send(Event::Usage { conv: conv.id, usage: Usage::jev(jev_tokens) });
    }

    let reads = Reads::from_history(&conv.messages[visible..]);
    let images = conv.messages.last().is_some_and(|m| !m.images.is_empty());
    // Gemini via Antigravity answers in text only: a turn that needs tools must run on another engine.
    ensure!(
        env.llm.tools() || route.tools().is_empty(),
        "Gemini (Antigravity) répond en texte seul : choisissez Claude ou DeepSeek pour un travail qui lit ou modifie des fichiers."
    );
    let mut turn = Turn { emit, turn, system: &system, reads: &reads, live: &mut live };
    if delegated(&route, &env.settings) {
        // Gemini Flash first, DeepSeek Flash next; one out of quota (or down) before answering hands over.
        // A text-only helper (Gemini via Antigravity) takes simple questions, never tool work.
        let usable = |h: &&Llm| !h.cooling() && (!images || (h.engine.vision() && h.tools())) && (h.tools() || route.delegable());
        for helper in env.helpers.iter().filter(usable) {
            let before = conv.messages.len();
            let tuned = routed_effort(helper, &env.settings, &route).unwrap_or_else(|| helper.clone());
            match steps(conv, &env.with_llm(tuned), &mut turn, env.settings.step_limit()).await {
                Err(e) if conv.messages.len() == before => helper.note_failure(&e),
                done => return finish(done?, env, &mut turn).await,
            }
        }
    }
    let main = routed_effort(&env.llm, &env.settings, &route).unwrap_or_else(|| env.llm.clone());
    if let Some(cheaper) = tiered(&main, env, conv, &route) {
        // A cheaper tier that errors or runs out of its step budget hands the turn to the chosen model.
        if let Ok(Finish::Done) = steps(conv, &env.with_llm(cheaper), &mut turn, Some(TIER_STEPS)).await {
            return finish(Finish::Done, env, &mut turn).await;
        }
    }
    let done = steps(conv, &env.with_llm(main), &mut turn, env.settings.step_limit()).await?;
    finish(done, env, &mut turn).await
}

/// Per-turn context shared by every engine that works on it.
struct Turn<'a> {
    emit: &'a Emitter,
    turn: i64,
    system: &'a str,
    reads: &'a Reads,
    live: &'a mut LiveTurn,
}

#[derive(Debug, PartialEq, Eq)]
enum Finish {
    Done,
    /// The step budget ran out before a final answer.
    Budget,
}

/// The turn's end: the step limit is reported, and the live run of the last edit must be green.
async fn finish(done: Finish, env: &Env, turn: &mut Turn<'_>) -> Result<()> {
    if done == Finish::Budget {
        let limit = env.settings.step_limit().unwrap_or(u32::MAX);
        bail!("Limite de {limit} étapes atteinte — envoyez « continue » pour poursuivre.");
    }
    if let Some(outcome) = turn.live.verdict().await.filter(|o| !o.ok) {
        bail!("Tests en continu : échec après le dernier changement — envoyez « corrige » pour réparer.\n{}", outcome.summary);
    }
    Ok(())
}

/// Simple questions always; tests, comments and summaries when that delegation is on too.
fn delegated(route: &Route, settings: &Settings) -> bool {
    settings.delegate && (route.delegable() || (settings.delegate_routine && route.routine_task()))
}

/// `llm` (main engine or helper) at Jev's effort, when it is lower than the user's and changing it keeps the cache.
fn routed_effort(llm: &Llm, settings: &Settings, route: &Route) -> Option<Llm> {
    if !settings.auto_effort || !llm.effort_keeps_cache() {
        return None;
    }
    let effort = route.effort_cap(llm.effort);
    (effort != llm.effort).then(|| llm.with_effort(effort))
}

/// Claude at the tier the turn's difficulty calls for (Haiku, Sonnet), when it is below the chosen model and the
/// switch does not rewrite a large history into a cold cache.
fn tiered(main: &Llm, env: &Env, conv: &Conversation, route: &Route) -> Option<Llm> {
    // Only a real Jev estimate moves the tier: local heuristics would send every turn to the middle one.
    if !env.settings.auto_model || main.engine != Engine::Claude || route.source != Source::Jev || route.effort.is_none() {
        return None;
    }
    let model = tier::pick(&main.model, route.difficulty())?;
    let previous = conv.messages.iter().rev().find_map(|m| m.engine.filter(|e| *e == Engine::Claude).map(|_| m.model.as_str()));
    if !tier::switch_pays(&model, previous, conv.context) {
        return None;
    }
    let cheaper = main.with_model(&model);
    // The cheaper tier follows the effort estimate even when the chosen model's effort was not lowered.
    Some(routed_effort(&cheaper, &env.settings, route).unwrap_or(cheaper))
}

/// App tools leave the toolset once the `APP_IDLE_TURNS` turns before the current one never called them
/// and the current one does not need them.
fn prune_idle_apps(conv: &mut Conversation, route: &Route) {
    const APPS: [ToolKind; 3] = [ToolKind::CliAnything, ToolKind::Kits, ToolKind::AuditKit];
    let users: Vec<usize> = conv
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == crate::domain::chat::Role::User)
        .map(|(i, _)| i)
        .collect();
    // Too young a conversation: everything it loaded is still recent.
    let Some(&from) = users.len().checked_sub(APP_IDLE_TURNS + 1).filter(|i| *i > 0).and_then(|i| users.get(i)) else {
        return;
    };
    let recent: Vec<ToolKind> = conv.messages[from..]
        .iter()
        .flat_map(|m| m.tool_calls.iter().filter_map(|c| ToolKind::parse(&c.name)).chain(m.tools_added.iter().copied()))
        .collect();
    let wanted = route.tools();
    conv.toolset.retain(|k| !APPS.contains(k) || wanted.contains(k) || recent.contains(k));
}

/// Live test runner seen from one turn: which results the model has seen, when the agent last edited.
struct LiveTurn {
    runner: Option<Arc<Runner>>,
    seen: u64,
    edited: Option<Instant>,
}

impl LiveTurn {
    fn new(runner: Option<Arc<Runner>>) -> Self {
        let seen = runner.as_ref().map_or(0, |r| r.seq());
        Self { runner, seen, edited: None }
    }

    /// The suite is red as the turn starts: the model hears it before it plans.
    fn failing_now(&self) -> Option<String> {
        let last = self.runner.as_ref()?.status().last?;
        (!last.ok).then(|| last.note())
    }

    fn edited(&mut self) {
        if let Some(r) = &self.runner {
            r.touch();
            self.edited = Some(Instant::now());
        }
    }

    /// A result the model has not seen yet.
    fn note(&mut self) -> Option<String> {
        let outcome = self.runner.as_ref()?.newer(self.seen)?;
        self.seen = outcome.seq;
        Some(outcome.note())
    }

    /// Result of the run started by the last edit of the turn (`None`: no edit, no runner, or no answer in time).
    async fn verdict(&mut self) -> Option<livetest::Outcome> {
        let (runner, edited) = (self.runner.as_ref()?, self.edited?);
        let outcome = runner.wait_after(edited, LIVE_WAIT).await?;
        self.seen = self.seen.max(outcome.seq);
        Some(outcome)
    }
}

/// The tool loop: stream, run the calls, repeat until an answer without calls or `limit` steps (`None` = no limit).
async fn steps(conv: &mut Conversation, env: &Env, turn: &mut Turn<'_>, limit: Option<u32>) -> Result<Finish> {
    let ctx = ToolCtx { env, root: env.workspace(), emit: turn.emit, conv: conv.id, turn: turn.turn, reads: turn.reads };
    let mut step = 0;
    let (engine, model, effort) = (env.llm.engine, env.llm.model.clone(), env.llm.effort);
    turn.emit.send(Event::Running { conv: conv.id, engine, model, effort });
    loop {
        if limit.is_some_and(|l| step >= l.max(1)) {
            return Ok(Finish::Budget);
        }
        step += 1;
        clear_if_over_budget(conv, env, turn.emit, turn.reads);
        let body = context::request(conv, env, turn.system, true);
        let done = match complete(env, turn.emit, conv.id, &body).await {
            // Mid-conversation tool changes refused (model, account or beta): classic tools for the rest of the session.
            Err(e) if llm::uses_deferred_tools(&body) && llm::tools_beta_rejected(&e) => {
                env.llm.disable_deferred();
                conv.prefix = wire_prefix(env, turn.system, conv);
                conv.thinking_from = conv.messages.len();
                complete(env, turn.emit, conv.id, &context::request(conv, env, turn.system, false)).await?
            }
            // Reasoning bound to an older state of the conversation (or another model): retry without it.
            Err(e) if llm::thinking_rejected(&e) => {
                // Later steps of this turn keep only the reasoning they produce themselves.
                conv.thinking_from = conv.messages.len();
                complete(env, turn.emit, conv.id, &context::request(conv, env, turn.system, false)).await?
            }
            done => done?,
        };
        let sample = savings::measure(conv, env, &body, &done.usage);
        conv.savings.record(&sample);
        turn.emit.send(Event::Measured { conv: conv.id, sample, thinking_from: conv.thinking_from });
        // Prompt size drives compaction; estimated (~4 bytes/token) if the server sent no usage.
        conv.context = if done.usage.prompt > 0 { done.usage.prompt } else { body.to_string().len() as u64 / 4 };
        turn.emit.send(Event::Usage { conv: conv.id, usage: done.usage });
        turn.emit.send(Event::Context { conv: conv.id, tokens: conv.context });

        let calls = done.tool_calls.clone();
        let answer = Message { parts: done.parts, ..Message::assistant(done.content, done.reasoning, done.tool_calls) };
        commit(conv, turn.emit, answer.by(env.llm.engine, &env.llm.model));
        if calls.is_empty() {
            return Ok(Finish::Done);
        }
        run_tools(conv, &ctx, &calls, turn.live).await;
    }
}

/// Past the context budget mid-turn, the oldest bulky tool results leave the wire (compaction waits for the next turn).
fn clear_if_over_budget(conv: &mut Conversation, env: &Env, emit: &Emitter, reads: &Reads) {
    let indices = clear::plan(conv, env.settings.compact_at, conv.savings.ratio());
    if indices.is_empty() {
        return;
    }
    clear::apply(conv, &indices);
    let visible = conv.summary.as_ref().map_or(0, |s| s.upto).min(conv.messages.len());
    reads.refresh(&conv.messages[visible..]);
    emit.send(Event::Cleared { conv: conv.id, indices });
}

/// What signed reasoning is bound to besides the history: the system prompt and the `tools` array actually sent.
fn wire_prefix(env: &Env, system: &str, conv: &Conversation) -> u64 {
    if env.llm.deferred_tools() {
        savings::prefix(system, &ToolKind::ALL.into_iter().collect())
    } else {
        savings::prefix(system, &conv.toolset)
    }
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
/// The last output of the step carries the diagnostics of any file change and the newest live test result.
async fn run_tools(conv: &mut Conversation, ctx: &ToolCtx<'_>, calls: &[ToolCall], live: &mut LiveTurn) {
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
    if !changed.is_empty() {
        live.edited();
    }
    // A build manifest or build script changed after web content: compiling would run it unasked.
    let unsafe_build = !ctx.env.settings.bypass && ctx.reads.untrusted() && changed.iter().any(|p| diagnostics::builds(p));
    if unsafe_build {
        output.text.push_str("

[diagnostics] non lancés : script de build ou manifeste modifié après lecture de contenu web — l'utilisateur doit valider la compilation.");
    } else if ctx.env.settings.auto_diagnostics
        && !changed.is_empty()
        && let Some(report) = diagnostics::check(&ctx.root, &changed).await
    {
        output.text.push_str("\n\n");
        output.text.push_str(&report);
    }
    if let Some(note) = live.note() {
        output.text.push_str("\n\n");
        output.text.push_str(&note);
    }
    if let Some(note) = ctx.env.inbox.note(conv.id) {
        output.text.push_str("\n\n");
        output.text.push_str(&note);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{engine::Effort, skill::Skill};

    fn simple() -> Route {
        Route {
            skill: Skill::Chat,
            source: Source::Jev,
            confidence: 0.9,
            web: 0.0,
            code: 0.0,
            risk: 0.0,
            blast: 0.0,
            apps: 0.0,
            playbook: None,
            effort: None,
            simple: 0.95,
            routine: 0.0,
            design: 0.0,
        }
    }

    #[test]
    fn simple_and_routine_turns_are_delegated_when_allowed() {
        let s = Settings::default();
        assert!(delegated(&simple(), &s));
        assert!(!delegated(&simple(), &Settings { delegate: false, ..s.clone() }));
        assert!(!delegated(&Route { code: 0.9, simple: 0.0, ..simple() }, &s));
        let tests = Route { code: 0.9, simple: 0.0, routine: 0.9, skill: Skill::Tdd, ..simple() };
        assert!(delegated(&tests, &s), "tests délégués");
        assert!(!delegated(&tests, &Settings { delegate_routine: false, ..s.clone() }));
        assert!(!delegated(&Route { risk: 0.9, ..tests }, &s), "sensible : jamais délégué");
    }

    #[test]
    fn the_delegated_helper_follows_the_effort_jev_estimated() {
        let s = Settings::default();
        let http = crate::infra::http::client("").unwrap();
        let helper = Llm::new(http, &s, Engine::DeepSeek).with_effort(Effort::High);
        let easy = Route { effort: Some(0.1), ..simple() };
        assert_eq!(routed_effort(&helper, &s, &easy).map(|l| l.effort), Some(Effort::Low));
        assert!(routed_effort(&helper, &Settings { auto_effort: false, ..s.clone() }, &easy).is_none());
        let hard = Route { effort: Some(0.9), ..simple() };
        assert!(routed_effort(&helper, &s, &hard).is_none(), "jamais au-dessus du choix de l'utilisateur");
    }

    fn claude_env(auto_model: bool) -> Env {
        let settings = Settings {
            engine: Engine::Claude,
            claude_auth: crate::domain::auth::AuthMode::ApiKey,
            claude_key: "k".into(),
            claude_model: "claude-opus-5-5".into(),
            auto_model,
            ..Settings::default()
        };
        let web = Arc::new(crate::tools::webindex::WebIndex::open(&std::env::temp_dir().join("omega-agent-web")));
        Env::new(settings, web).unwrap()
    }

    #[test]
    fn jev_difficulty_picks_the_claude_tier() {
        let env = claude_env(true);
        let conv = Conversation::default();
        let easy = Route { effort: Some(0.1), ..simple() };
        assert_eq!(tiered(&env.llm, &env, &conv, &easy).map(|l| l.model), Some("claude-haiku-5-5".into()));
        let medium = Route { effort: Some(0.5), simple: 0.0, code: 0.8, ..simple() };
        assert_eq!(tiered(&env.llm, &env, &conv, &medium).map(|l| l.model), Some("claude-sonnet-5-5".into()));
        let hard = Route { effort: Some(0.9), simple: 0.0, ..simple() };
        assert!(tiered(&env.llm, &env, &conv, &hard).is_none(), "difficile : le modèle choisi");
        let off = claude_env(false);
        assert!(tiered(&off.llm, &off, &conv, &easy).is_none(), "option désactivée");
        assert!(tiered(&env.llm, &env, &conv, &Route { source: Source::Local, ..easy.clone() }).is_none(), "sans Jev");
        let big = Conversation { context: 120_000, ..Conversation::default() };
        assert!(tiered(&env.llm, &env, &big, &easy).is_none(), "gros historique, cache froid : pas de bascule");
    }

    #[test]
    fn idle_app_tools_leave_the_toolset() {
        let mut conv = Conversation::default();
        conv.toolset.extend([ToolKind::ReadFile, ToolKind::CliAnything]);
        let call = ToolCall { id: "c".into(), name: "cli_anything".into(), ..ToolCall::default() };
        conv.push(Message::user("blender".into()));
        conv.push(Message::assistant(String::new(), String::new(), vec![call]));
        prune_idle_apps(&mut conv, &simple());
        assert!(conv.toolset.contains(&ToolKind::CliAnything), "conversation trop jeune");
        for i in 0..APP_IDLE_TURNS {
            conv.push(Message::user(format!("question {i}")));
            conv.push(Message::assistant("ok".into(), String::new(), Vec::new()));
        }
        conv.push(Message::user("tour courant".into()));
        let mut needs = conv.clone();
        prune_idle_apps(&mut needs, &Route { apps: 0.9, ..simple() });
        assert!(needs.toolset.contains(&ToolKind::CliAnything), "encore demandé par ce tour");
        prune_idle_apps(&mut conv, &simple());
        assert!(!conv.toolset.contains(&ToolKind::CliAnything), "inutilisé depuis {APP_IDLE_TURNS} tours");
        assert!(conv.toolset.contains(&ToolKind::ReadFile), "seuls les outils d'applications partent");
    }
}
