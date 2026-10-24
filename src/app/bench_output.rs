//! Live checks of the output-side levers (output discipline A/B, DeepSeek's thinking switch) and of mid-turn clearing.

use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};

use super::{
    Env,
    bench::{answer_tokens, used},
    context, prompts,
};
use crate::{
    domain::{
        chat::{Conversation, Message, ToolCall},
        engine::{Effort, Engine},
        tool::ToolKind,
    },
    infra::llm::Llm,
    tools::schema,
};

/// The 0.6.0 output section: the A side of the A/B.
const BEFORE: &str = "# Output\nCode blocks with language tag. Only changed parts unless the file is new. File refs as path:line.";
const TASK: &str = "Lis src/a.rs et src/b.rs : lequel définit le plus de fonctions ?";
const MAX_STEPS: usize = 4;

#[derive(Default)]
struct Run {
    steps: usize,
    calls: usize,
    visible: u64,
    billed: u64,
}

/// The same two-file task through the real request path, without then with the output discipline.
pub async fn discipline(env: &Env, llm: &Llm, spent: &mut u64) -> Result<String> {
    let env = env.with_llm(llm.clone());
    let system = prompts::system(&env.settings, &env.workspace());
    ensure!(system.contains(prompts::OUTPUT), "section # Output absente du prompt système");
    let a = tool_loop(&env, &system.replace(prompts::OUTPUT, BEFORE), spent).await?;
    let b = tool_loop(&env, &system, spent).await?;
    ensure!(b.visible <= a.visible, "pas plus court avec la discipline ({} vs {} tokens visibles)", b.visible, a.visible);
    Ok(format!(
        "texte visible {} → {} tok (−{} %) · étapes {} → {} · appels {} → {} · sortie facturée {} → {} (raisonnement inclus) · 1 échantillon",
        a.visible,
        b.visible,
        (a.visible - b.visible) * 100 / a.visible.max(1),
        a.steps,
        b.steps,
        a.calls,
        b.calls,
        a.billed,
        b.billed
    ))
}

async fn tool_loop(env: &Env, system: &str, spent: &mut u64) -> Result<Run> {
    let mut conv = Conversation::default();
    conv.toolset.extend(ToolKind::READ);
    conv.push(Message::user(TASK.into()));
    let mut run = Run::default();
    for _ in 0..MAX_STEPS {
        let body = context::request(&conv, env, system, true);
        let done = env.llm.stream(&body, |_| {}).await?;
        run.steps += 1;
        run.billed += used(spent, &done).completion;
        run.visible += answer_tokens(&env.llm, &done, spent).await?;
        run.calls += done.tool_calls.len();
        let results: Vec<Message> = done.tool_calls.iter().map(|c| Message::tool(c.id.clone(), fake(c))).collect();
        conv.push(Message { parts: done.parts, ..Message::assistant(done.content, done.reasoning, done.tool_calls) });
        if results.is_empty() {
            return Ok(run);
        }
        results.into_iter().for_each(|m| conv.push(m));
    }
    Ok(run)
}

/// Canned tool results: a.rs defines three functions, b.rs one; anything else lists both files.
fn fake(call: &ToolCall) -> String {
    let args = call.arguments.as_str();
    if args.contains("a.rs") {
        "fn alpha() {}\nfn beta() {}\nfn gamma() {}".into()
    } else if args.contains("b.rs") {
        "fn delta() {}".into()
    } else {
        "src/a.rs\nsrc/b.rs".into()
    }
}

/// A real tool loop whose earlier results are then cleared: the provider must accept the stubbed history
/// (Claude's signed reasoning included) and the turn must go on.
pub async fn clearing(env: &Env, llm: &Llm, spent: &mut u64) -> Result<String> {
    let env = env.with_llm(llm.clone());
    let system = prompts::system(&env.settings, &env.workspace());
    let mut conv = Conversation::default();
    conv.toolset.extend(ToolKind::READ);
    conv.push(Message::user(TASK.into()));
    let done = env.llm.stream(&context::request(&conv, &env, &system, true), |_| {}).await?;
    used(spent, &done);
    ensure!(!done.tool_calls.is_empty(), "aucun appel d'outil au 1er pas : effacement non testable");
    let padded: Vec<Message> =
        done.tool_calls.iter().map(|c| Message::tool(c.id.clone(), format!("{}\n{}", fake(c), "// filler\n".repeat(400)))).collect();
    conv.push(Message { parts: done.parts, ..Message::assistant(done.content, done.reasoning, done.tool_calls) });
    let first = conv.messages.len();
    padded.into_iter().for_each(|m| conv.push(m));
    let indices: Vec<usize> = (first..conv.messages.len()).collect();
    // The model reads the full results and signs its reasoning over them; only then are they cleared.
    let seen = env.llm.stream(&context::request(&conv, &env, &system, true), |_| {}).await?;
    used(spent, &seen);
    let more: Vec<Message> = seen.tool_calls.iter().map(|c| Message::tool(c.id.clone(), fake(c))).collect();
    conv.push(Message { parts: seen.parts, ..Message::assistant(seen.content, seen.reasoning, seen.tool_calls) });
    if more.is_empty() {
        conv.push(Message::user("Réponds en un mot : lequel ?".into()));
    }
    more.into_iter().for_each(|m| conv.push(m));
    let before = context::request(&conv, &env, &system, true).to_string().len();
    super::clear::apply(&mut conv, &indices);
    let body = context::request(&conv, &env, &system, true);
    let after = body.to_string().len();
    let next = env.llm.stream(&body, |_| {}).await.context("historique effacé refusé par l'API")?;
    let usage = used(spent, &next);
    Ok(format!(
        "{} résultat(s) effacé(s) : requête {before} → {after} o · accepté par {} (raisonnement signé conservé) · {} tok lus du cache · le modèle {}",
        indices.len(),
        env.llm.model,
        usage.cached,
        if next.tool_calls.is_empty() { "répond" } else { "relit ce dont il a besoin" }
    ))
}

/// DeepSeek at Low then Off on a tool conversation whose history carries `reasoning_content`: Off must bill no thinking.
pub async fn deepseek_off(env: &Env) -> Result<String> {
    let ds = Llm::new(env.http.clone(), &env.settings, Engine::DeepSeek);
    if !ds.configured() {
        return Ok("DeepSeek non configuré : non mesuré".into());
    }
    let tools = Some(schema::schemas(&ToolKind::READ.iter().copied().collect()));
    let messages = history();
    let mut spent = 0;
    let (low, low_chars, low_answer) = thinking(&ds.with_effort(Effort::Low), &messages, tools.clone(), &mut spent).await?;
    let (off, off_chars, off_answer) = thinking(&ds.with_effort(Effort::Off), &messages, tools, &mut spent).await?;
    ensure!(off == 0 && off_chars == 0, "Off : {off} tokens de réflexion facturés ({off_chars} car.)");
    Ok(format!(
        "réflexion Low {low} tok ({low_chars} car.) → Off {off} tok · réponses « {low_answer} » / « {off_answer} » · historique outils + reasoning_content accepté · {spent} tokens {}",
        ds.model
    ))
}

/// (billed reasoning tokens, reasoning characters, answer) of one request.
async fn thinking(llm: &Llm, messages: &[Value], tools: Option<Value>, spent: &mut u64) -> Result<(u64, usize, String)> {
    let done = llm.stream(&llm.body(messages, tools), |_| {}).await?;
    used(spent, &done);
    Ok((done.usage.reasoning, done.reasoning.chars().count(), done.content.trim().to_owned()))
}

fn history() -> [Value; 6] {
    [
        json!({ "role": "system", "content": "You are a terse assistant." }),
        json!({ "role": "user", "content": "List the workspace." }),
        json!({
            "role": "assistant",
            "content": "",
            "reasoning_content": "I should list the directory.",
            "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "list_dir", "arguments": "{}" } }],
        }),
        json!({ "role": "tool", "tool_call_id": "c1", "content": "src/\nCargo.toml" }),
        json!({ "role": "assistant", "content": "2 entries.", "reasoning_content": "Two entries were listed." }),
        json!({ "role": "user", "content": "17 × 23 = ? Number only." }),
    ]
}
