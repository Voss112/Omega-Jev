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

/// After the process exits, its output gets this long to flush before background children holding the pipes are left alone.
const DRAIN: Duration = Duration::from_secs(2);

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
    run_masked(command, &ctx.root, timeout_secs.clamp(1, 1800), cmd, &secrets(&env)).await
}

/// git and gh authenticated with the connected GitHub account, when the user shares it with the agent.
fn github_env(ctx: &ToolCtx<'_>) -> Vec<(String, String)> {
    if !ctx.env.settings.github_shell {
        return Vec::new();
    }
    Auth::load(Provider::GitHub).map(|a| github::shell_env(&a.access())).unwrap_or_default()
}

/// What the model must never see, even if a command prints its environment: the token, the Basic header and its
/// base64 payload (the token, decoded).
fn secrets(env: &[(String, String)]) -> Vec<String> {
    env.iter()
        .filter(|(key, _)| matches!(key.as_str(), "GH_TOKEN" | "GIT_CONFIG_VALUE_0"))
        .flat_map(|(_, value)| {
            let payload = value.strip_prefix("AUTHORIZATION: basic ").map(str::to_owned);
            std::iter::once(value.clone()).chain(payload)
        })
        .filter(|s| s.len() > 8)
        .collect()
}

fn redact(mut out: String, secrets: &[String]) -> String {
    for secret in secrets {
        out = out.replace(secret.as_str(), "«secret»");
    }
    out
}

/// Security gate shared by every process the agent starts: deterministic rules → Jev → user approval.
pub async fn gate(ctx: &ToolCtx<'_>, cmd: &str) -> Result<()> {
    let reason = match guard::assess(cmd) {
        Verdict::Blocked(why) => bail!("blocked by security guard: {why}"),
        // Bypass: no prompt and no Jev second opinion (saves a call per command).
        _ if ctx.env.settings.unattended() => return Ok(()),
        Verdict::Risky(why) => Some(why.to_owned()),
        // Text the model may have been fed (a web page) can hide in a comment: Jev's opinion is not enough.
        Verdict::Unknown if cmd.contains('#') => Some("commentaire dans une commande non reconnue".to_owned()),
        // After outside content, code the agent may have just written does not run unasked.
        _ if ctx.reads.untrusted() && guard::runs_code(cmd) => {
            Some("exécute du code après lecture de contenu web (injection possible)".to_owned())
        }
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
pub async fn run_command(command: process::Command, dir: &Path, timeout_secs: usize, line: &str) -> Result<String> {
    run_masked(command, dir, timeout_secs, line, &[]).await
}

/// `run_command` with `secrets` masked in the raw output, before compression: a token clipped by the compressor
/// would slip past a mask applied afterwards.
async fn run_masked(mut command: process::Command, dir: &Path, timeout_secs: usize, line: &str, secrets: &[String]) -> Result<String> {
    command.current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().context("cannot start process")?;
    let (stdout, stderr) = (child.stdout.take().context("stdout")?, child.stderr.take().context("stderr")?);
    let mut tree = Tree { child, done: false };
    let limit = Duration::from_secs(timeout_secs as u64);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (status, detached) = {
        let reading = async { tokio::join!(capped(stdout, &mut out), capped(stderr, &mut err)) };
        tokio::pin!(reading);
        // The command is over when its process exits, not when its pipes close: a background child it started
        // (`Start-Process`, a dev server, a GUI) inherits them and would otherwise hold the call until the timeout.
        tokio::time::timeout(limit, async {
            tokio::select! {
                status = tree.child.wait() => {
                    let status = status?;
                    let drained = tokio::time::timeout(DRAIN, &mut reading).await;
                    let detached = match drained {
                        Ok((o, e)) => { o?; e?; false }
                        Err(_) => true,
                    };
                    anyhow::Ok((status, detached))
                }
                (o, e) = &mut reading => {
                    o?;
                    e?;
                    anyhow::Ok((tree.child.wait().await?, false))
                }
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("timeout after {timeout_secs}s (process killed)"))??
    };
    tree.done = true;
    let mut text = String::from_utf8_lossy(&out).into_owned();
    let stderr = String::from_utf8_lossy(&err);
    if !stderr.trim().is_empty() {
        text.push_str("\n[stderr]\n");
        text.push_str(&stderr);
    }
    if detached {
        text.push_str("\n(processus d'arrière-plan laissé en cours : sortie non attendue)");
    }
    let code = status.code().map_or_else(|| "killed".to_owned(), |c| c.to_string());
    Ok(format!("exit {code}\n{}", compress::command(line, &redact(text, secrets))))
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

/// Appends at most OUTPUT_CAP bytes to `buf` (kept even if the read is abandoned); dropping the pipe stops a runaway writer.
async fn capped(reader: impl AsyncRead + Unpin, buf: &mut Vec<u8>) -> std::io::Result<()> {
    let mut reader = reader.take(OUTPUT_CAP);
    let mut chunk = [0u8; 8192];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

pub async fn probe() -> Result<String> {
    let out = execute(&std::env::temp_dir(), "echo omega", 30).await?;
    ensure!(out.contains("omega"), "sortie inattendue");
    Ok(process::shell_name().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_github_token_and_its_basic_payload_are_masked() {
        let env = github::shell_env("gho_0123456789abcdef");
        let hidden = secrets(&env);
        let header = env.iter().find(|(k, _)| k == "GIT_CONFIG_VALUE_0").unwrap().1.clone();
        let payload = header.trim_start_matches("AUTHORIZATION: basic ").to_owned();
        let out = redact(format!("t=gho_0123456789abcdef h={header} b={payload}"), &hidden);
        assert!(!out.contains("gho_0123456789abcdef") && !out.contains(&payload), "{out}");
        assert_eq!(out.matches("«secret»").count(), 3, "{out}");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_background_child_does_not_hold_the_call_until_timeout() {
        // `cmd /c start /b` leaves a child holding the inherited pipes for 20 s.
        let started = std::time::Instant::now();
        let out = execute(&std::env::temp_dir(), "cmd /c \"start /b ping -n 20 127.0.0.1 >nul\"; echo fini", 15).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        assert!(out.starts_with("exit 0"), "{out}");
        assert!(out.contains("fini"), "{out}");
    }
}