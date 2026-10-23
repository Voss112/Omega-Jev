//! File changes are planned (pure, sync) then applied here: diff → optional approval → checkpoint → write.

use std::{fmt::Write as _, fs, path::PathBuf};

use anyhow::{Result, ensure};
use similar::{ChangeTag, TextDiff};

use super::{ToolCtx, ToolOutput, approve, blocking, checkpoint::Checkpoint};
use crate::app::ApprovalKind;

pub struct Change {
    pub path: PathBuf,
    pub rel: String,
    pub before: Option<String>,
    pub after: String,
    /// Short verb phrase for the model, e.g. "edited src/a.rs".
    pub action: String,
}

const MAX_DIFF_LINES: usize = 400;

pub async fn apply(ctx: &ToolCtx<'_>, change: Change) -> Result<ToolOutput> {
    let before = change.before.as_deref().unwrap_or_default();
    let diff = TextDiff::from_lines(before, &change.after);
    let (mut added, mut removed) = (0, 0);
    for c in diff.iter_all_changes() {
        match c.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    let unified = clip_lines(&diff.unified_diff().context_radius(2).header(&change.rel, &change.rel).to_string());
    if ctx.env.settings.confirm_edits {
        let reason = format!("Modification de {} (+{added} −{removed})", change.rel);
        ensure!(approve(ctx, ApprovalKind::Edit, unified.clone(), reason).await, "change refused by the user");
    }
    let checkpoint = Checkpoint::new(ctx.conv, ctx.turn);
    let (path, after) = (change.path.clone(), change.after);
    blocking(move || {
        checkpoint.record(&path)?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, after)?;
        Ok(())
    })
    .await?;
    Ok(ToolOutput { text: format!("{} (+{added} -{removed})", change.action), diff: Some(unified), changed: Some(change.path) })
}

fn clip_lines(diff: &str) -> String {
    let total = diff.lines().count();
    if total <= MAX_DIFF_LINES {
        return diff.to_owned();
    }
    let mut out: String = diff.lines().take(MAX_DIFF_LINES).collect::<Vec<_>>().join("\n");
    let _ = write!(out, "\n… {} more diff lines", total - MAX_DIFF_LINES);
    out
}
