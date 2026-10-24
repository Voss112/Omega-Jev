//! Guarded shell: deterministic classifier → Jev second opinion → user approval → compressed output.

use std::{
    collections::HashMap,
    path::Path,
    process::Stdio,
    sync::{LazyLock, Mutex, PoisonError},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt};

use super::{
    ToolCtx, approve, compress,
    guard::{self, Verdict},
    livetest,
};
use crate::{
    app::{ApprovalKind, Event},
    domain::{auth::Provider, chat::Usage},
    infra::{
        auth::{Auth, github},
        jev::Question,
        process,
    },
};

const OUTPUT_CAP: u64 = 2 << 20;

/// A test command waits this long for the live watcher's run instead of starting a second one.
const LIVE_WAIT: Duration = Duration::from_secs(180);

pub async fn run(ctx: &ToolCtx<'_>, cmd: &str, timeout_secs: usize) -> Result<String> {
    // Tests already run continuously: their latest result answers, the suite is not run a second time.
    if ctx.env.settings.live_tests
        && livetest::is_test_command(cmd)
        && let Some(runner) = livetest::current(&ctx.root)
        && let Some(outcome) = runner.latest(LIVE_WAIT).await
    {
        let code = if outcome.ok { 0 } else { 1 };
        return Ok(format!("exit {code}
{}
(résultat du watcher en continu : pas de seconde exécution)", outcome.note()));
    }
    gate(ctx, cmd).await?;
    let env = github_env(ctx);
    let mut command = process::line(cmd);
    command.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let out = run_command(command, &ctx.root, timeout_secs.clamp(1, 1800), cmd).await?;
    Ok(redact(out, &env))
}

/// git and gh authenticated with the connected GitHub account, when the user shares it with the agent.
fn github_env(ctx: &ToolCtx<'_>) -> Vec<(String, String)> {
    if !ctx.env.settings.github_shell {
        return Vec::new();
    }
    Auth::load(Provider::GitHub).map(|a| github::shell_env(&a.access())).unwrap_or_default()
}

/// The token never reaches the model, even if a command prints its environment.
fn redact(mut out: String, env: &[(String, String)]) -> String {
    for (key, value) in env {
        if matches!(key.as_str(), "GH_TOKEN" | "GIT_CONFIG_VALUE_0") && value.len() > 8 {
            out = out.replace(value.as_str(), "«secret»");
        }
    }
    out
}

/// Security gate shared by every process the agent starts: deterministic rules → Jev → user approval.
pub async fn gate(ctx: &ToolCtx<'_>, cmd: &str) -> Result<()> {
    let reason = match guard::assess(cmd) {
        Verdict::Blocked(why) => bail!("blocked by security guard: {why}"),
        // Bypass: no prompt and no Jev second opinion (saves a call per command).
        _ if ctx.env.settings.bypass => return Ok(()),
        Verdict::Risky(why) => Some(why.to_owned()),
        Verdict::Unknown => cached_verdict(ctx, cmd).await,
        Verdict::Safe => None,
    };
    let reason = reason.or_else(|| (!ctx.env.settings.auto_approve).then(|| "approbation manuelle activée".to_owned()));
    if let Some(reason) = reason {
        ensure!(approve(ctx, ApprovalKind::Command, cmd.to_owned(), reason).await, "command refused by the user");
    }
    Ok(())
}

/// Jev's verdict per command line: a command repeated in the session (a build, a test, a git call) costs no call.
static VERDICTS: LazyLock<Mutex<HashMap<String, Option<String>>>> = LazyLock::new(Default::default);

async fn cached_verdict(ctx: &ToolCtx<'_>, cmd: &str) -> Option<String> {
    let key = cmd.trim().to_owned();
    if let Some(hit) = VERDICTS.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
        return hit.clone();
    }
    let (verdict, answered) = jev_verdict(ctx, cmd).await;
    if answered {
        let mut cache = VERDICTS.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.len() >= 512 {
            cache.clear();
        }
        cache.insert(key, verdict.clone());
    }
    verdict
}

/// `Some(reason)` when the command needs approval; `true` when Jev actually answered (cacheable).
async fn jev_verdict(ctx: &ToolCtx<'_>, cmd: &str) -> (Option<String>, bool) {
    let Some(jev) = &ctx.env.jev else { return (Some("commande non reconnue".into()), false) };
    let q = [(
        "danger".to_owned(),
        Question::noul(
            "Could this shell command destroy data, change system state outside the project, leak secrets or exfiltrate data?",
        ),
    )];
    match jev.ask(json!({ "command": cmd, "os": std::env::consts::OS }), &q).await {
        Ok(a) => {
            ctx.emit.send(Event::Usage { conv: ctx.conv, usage: Usage::jev(a.tokens) });
            let p = a.noul("danger").unwrap_or(1.0);
            ((p >= 0.35).then(|| format!("Jev : risque {:.0} %", p * 100.0)), true)
        }
        Err(_) => (Some("commande non reconnue (Jev indisponible)".into()), false),
    }
}

pub async fn execute(dir: &Path, cmd: &str, timeout_secs: usize) -> Result<String> {
    run_command(process::line(cmd), dir, timeout_secs, cmd).await
}

/// Runs a prepared command (no shell involved unless the caller built one): capped, timed, compressed.
/// `line` is the command as typed, which picks the RTK filter (empty = generic only).
pub async fn run_command(mut command: process::Command, dir: &Path, timeout_secs: usize, line: &str) -> Result<String> {
    command.current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().context("cannot start process")?;
    let (stdout, stderr) = (child.stdout.take().context("stdout")?, child.stderr.take().context("stderr")?);
    let mut tree = Tree { child, done: false };
    let run = async {
        let (out, err) = tokio::join!(capped(stdout), capped(stderr));
        let status = tree.child.wait().await?;
        tree.done = true;
        anyhow::Ok((out?, err?, status))
    };
    // On timeout (or when the turn is stopped) the tree is dropped: the process and its children are killed.
    let (out, err, status) = tokio::time::timeout(Duration::from_secs(timeout_secs as u64), run)
        .await
        .map_err(|_| anyhow::anyhow!("timeout after {timeout_secs}s (process killed)"))??;
    let mut text = String::from_utf8_lossy(&out).into_owned();
    let stderr = String::from_utf8_lossy(&err);
    if !stderr.trim().is_empty() {
        text.push_str("\n[stderr]\n");
        text.push_str(&stderr);
    }
    let code = status.code().map_or_else(|| "killed".to_owned(), |c| c.to_string());
    Ok(format!("exit {code}\n{}", compress::command(line, &text)))
}

/// A running child that takes its whole process tree down when dropped unfinished: `kill_on_drop` alone only
/// reaches the shell, leaving `cargo`, a dev server or a watcher running after a timeout or a stop.
struct Tree {
    child: process::Child,
    done: bool,
}

impl Drop for Tree {
    fn drop(&mut self) {
        let Some(pid) = self.child.id().filter(|_| !self.done) else { return };
        // Runs before the field drop (`kill_on_drop`), while the parent still links its children.
        kill_tree(pid);
    }
}

/// Kills `pid` and every process it started (a watcher, a dev server, a build).
pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    let kill = process::blocking("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).output();
    #[cfg(not(windows))]
    let kill = process::blocking("kill").args(["-KILL", "--", &format!("-{pid}")]).output();
    let _ = kill;
}

/// Reads at most OUTPUT_CAP bytes; dropping the pipe afterwards stops a runaway writer.
async fn capped(reader: impl AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    reader.take(OUTPUT_CAP).read_to_end(&mut buf).await?;
    Ok(buf)
}

pub async fn probe() -> Result<String> {
    let out = execute(&std::env::temp_dir(), "echo omega", 30).await?;
    ensure!(out.contains("omega"), "sortie inattendue");
    Ok(process::shell_name().into())
}
