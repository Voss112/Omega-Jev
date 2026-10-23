//! Text search with ripgrep's own engine (grep-searcher + ignore), in-process.

use std::{fmt::Write as _, fs, path::Path};

use anyhow::{Result, ensure};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder, sinks::UTF8};

use super::{
    code::rel_path,
    compress::{clip, toon},
    fs as wfs,
};

/// Searches `base` (workspace or a dependency source dir); paths are shown relative to `root` when inside it.
pub fn grep(root: &Path, base: &Path, pattern: &str, glob: Option<&str>, max: usize) -> Result<String> {
    let matcher = RegexMatcherBuilder::new().case_smart(true).build(pattern)?;
    let mut searcher = SearcherBuilder::new().binary_detection(BinaryDetection::quit(0)).line_number(true).build();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut total = 0usize;
    for path in wfs::files(base, glob)? {
        let rel = rel_path(root, &path);
        let _ = searcher.search_path(
            &matcher,
            &path,
            UTF8(|line_no, line| {
                total += 1;
                if rows.len() < max {
                    rows.push(vec![rel.clone(), line_no.to_string(), clip(line.trim(), 200)]);
                }
                Ok(true)
            }),
        );
    }
    if rows.is_empty() {
        return Ok("no matches".into());
    }
    let mut out = toon("matches", &["file", "line", "text"], &rows);
    if total > max {
        let _ = write!(out, "\n(+{} more; refine pattern or glob)", total - max);
    }
    Ok(out)
}

pub fn self_test() -> Result<String> {
    let dir = std::env::temp_dir().join("omega-selftest-grep");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("probe.txt"), "alpha\nneedle_42\nomega\n")?;
    let out = grep(&dir, &dir, r"needle_\d+", None, 5)?;
    ensure!(out.contains("probe.txt,2,needle_42"), "résultat inattendu");
    Ok("grep-searcher + ignore (gitignore)".into())
}
