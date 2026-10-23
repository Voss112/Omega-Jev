//! Guarded shell: deterministic classifier → Jev second opinion → user approval → compressed output.

use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt};

use super::{
    ToolCtx, approve, compress,
    guard::{self, Verdict},
};
use crate::{
    app::{ApprovalKind, Event},
    domain::chat::Usage,
    infra::{jev::Question, process},
};

const OUTPUT_CAP: u64 = 2 << 20;

pub async fn run(ctx: &ToolCtx<'_>, cmd: &str, timeout_secs: usize) -> Result<String> {
    gate(ctx, cmd).await?;
    execute(&ctx.root, cmd, timeout_secs.clamp(1, 1800)).await
}

/// Security gate shared by every process the agent starts: deterministic rules → Jev → user approval.
pub async fn gate(ctx: &ToolCtx<'_>, cmd: &str) -> Result<()> {
    let reason = match guard::assess(cmd) {
        Verdict::Blocked(why) => bail!("blocked by security guard: {why}"),
        // Bypass: no prompt and no Jev second opinion (saves a call per command).
        _ if ctx.env.settings.bypass => return Ok(()),
        Verdict::Risky(why) => Some(why.to_owned()),
        Verdict::Unknown => jev_verdict(ctx, cmd).await,
        Verdict::Safe => None,
    };
    let reason = reason.or_else(|| (!ctx.env.settings.auto_approve).then(|| "approbation manuelle activée".to_owned()));
    if let Some(reason) = reason {
        ensure!(approve(ctx, ApprovalKind::Command, cmd.to_owned(), reason).await, "command refused by the user");
    }
    Ok(())
}

/// `Some(reason)` when the command needs approval.
async fn jev_verdict(ctx: &ToolCtx<'_>, cmd: &str) -> Option<String> {
    let Some(jev) = &ctx.env.jev else { return Some("commande non reconnue".into()) };
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
            (p >= 0.35).then(|| format!("Jev : risque {:.0} %", p * 100.0))
        }
        Err(_) => Some("commande non reconnue (Jev indisponible)".into()),
    }
}

pub async fn execute(dir: &Path, cmd: &str, timeout_secs: usize) -> Result<String> {
    run_command(process::shell(cmd), dir, timeout_secs).await
}

/// Runs a prepared command (no shell involved unless the caller built one): capped, timed, compressed.
pub async fn run_command(mut command: process::Command, dir: &Path, timeout_secs: usize) -> Result<String> {
    command.current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().context("cannot start process")?;
    let (stdout, stderr) = (child.stdout.take().context("stdout")?, child.stderr.take().context("stderr")?);
    let run = async move {
        let (out, err) = tokio::join!(capped(stdout), capped(stderr));
        let status = child.wait().await?;
        anyhow::Ok((out?, err?, status))
    };
    // On timeout the future (and the child, kill_on_drop) is dropped → process killed.
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
    Ok(format!("exit {code}\n{}", compress::shell(&text)))
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
