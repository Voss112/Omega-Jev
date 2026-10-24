//! Sandboxed file access, gitignore-aware walking.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use ignore::{WalkBuilder, overrides::OverrideBuilder};

use super::{change::Change, compress::clip, guard};

const SKIP_DIRS: &[&str] =
    &["target", "node_modules", ".git", "dist", "build", "__pycache__", ".venv", "venv", ".next", ".idea", ".vs"];
const DEFAULT_LINES: usize = 200;
const MAX_LINES: usize = 800;

fn walker(dir: &Path) -> WalkBuilder {
    let mut b = WalkBuilder::new(dir);
    b.hidden(true)
        .require_git(false)
        .max_filesize(Some(2 << 20))
        // Same tree → same bytes: tool outputs stay deterministic (and cache-friendly).
        .sort_by_file_name(std::ffi::OsStr::cmp)
        .filter_entry(|e| !e.file_name().to_str().is_some_and(|n| SKIP_DIRS.contains(&n)));
    b
}

/// Every non-ignored file under `dir`, optionally filtered by a glob.
pub fn files(dir: &Path, glob: Option<&str>) -> Result<impl Iterator<Item = PathBuf>> {
    let mut b = walker(dir);
    if let Some(g) = glob {
        let mut ob = OverrideBuilder::new(dir);
        ob.add(g).context("invalid glob")?;
        b.overrides(ob.build()?);
    }
    Ok(b.build().flatten().filter(|e| e.file_type().is_some_and(|t| t.is_file())).map(ignore::DirEntry::into_path))
}

pub fn tree(dir: &Path, depth: usize, max: usize) -> String {
    let mut b = walker(dir);
    b.max_depth(Some(depth)).sort_by_file_path(std::cmp::Ord::cmp);
    let mut lines = Vec::new();
    let mut total = 0;
    for e in b.build().flatten().filter(|e| e.depth() > 0) {
        total += 1;
        if lines.len() < max {
            let slash = if e.file_type().is_some_and(|t| t.is_dir()) { "/" } else { "" };
            lines.push(format!("{}{}{slash}", "  ".repeat(e.depth() - 1), e.file_name().to_string_lossy()));
        }
    }
    if total > max {
        lines.push(format!("… +{} entries", total - max));
    }
    if lines.is_empty() { "(empty)".into() } else { lines.join("\n") }
}

pub fn list(root: &Path, rel: &str, depth: usize) -> Result<String> {
    let dir = guard::resolve_read(root, rel)?;
    ensure!(dir.is_dir(), "not a directory: {rel}");
    Ok(tree(&dir, depth, 250))
}

pub fn read_text(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    ensure!(!bytes[..bytes.len().min(8000)].contains(&0), "binary file");
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Lines `start..=end` (1-based) prefixed with their numbers.
pub fn numbered(text: &str, start: usize, end: usize) -> String {
    let mut out = String::new();
    for (i, line) in text.lines().enumerate().skip(start.saturating_sub(1)).take(end.saturating_sub(start) + 1) {
        let _ = writeln!(out, "{:>5}  {}", i + 1, clip(line, 400));
    }
    out
}

pub fn read(root: &Path, rel: &str, start: Option<usize>, end: Option<usize>) -> Result<String> {
    let text = read_text(&guard::resolve_read(root, rel)?)?;
    let total = text.lines().count();
    let start = start.unwrap_or(1).max(1);
    ensure!(start <= total.max(1), "start {start} beyond end of file ({total} lines)");
    let end = end.unwrap_or(start + DEFAULT_LINES - 1).min(total).min(start + MAX_LINES - 1);
    let mut out = numbered(&text, start, end);
    if end < total {
        let _ = write!(out, "… showing {start}-{end} of {total} lines; use start/end");
    }
    Ok(out)
}

pub fn plan_write(root: &Path, rel: &str, content: String) -> Result<Change> {
    let path = guard::resolve_writable(root, rel)?;
    ensure!(!path.is_dir(), "{rel} is a directory");
    let before = path.is_file().then(|| read_text(&path)).transpose()?;
    let verb = if before.is_some() { "overwrote" } else { "created" };
    Ok(Change { action: format!("{verb} {rel}"), rel: rel.to_owned(), path, before, after: content })
}

pub fn plan_edit(root: &Path, rel: &str, old: &str, new: &str, all: bool) -> Result<Change> {
    let path = guard::resolve_writable(root, rel)?;
    let text = read_text(&path)?;
    ensure!(!old.is_empty(), "`old` is empty");
    let (old, new) = if text.contains("\r\n") && !old.contains('\r') {
        (old.replace('\n', "\r\n"), new.replace('\n', "\r\n"))
    } else {
        (old.to_owned(), new.to_owned())
    };
    let count = text.matches(old.as_str()).count();
    ensure!(count > 0, "`old` not found in {rel}; re-read the file, whitespace must match exactly");
    ensure!(count == 1 || all, "`old` matches {count} times; add context or set all=true");
    let after = if all { text.replace(&old, &new) } else { text.replacen(&old, &new, 1) };
    Ok(Change { action: format!("edited {rel}: {count} replacement(s)"), rel: rel.to_owned(), path, before: Some(text), after })
}
