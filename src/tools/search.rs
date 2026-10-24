//! Text search with ripgrep's own engine (grep-searcher + ignore), in-process, one searcher per core.

use std::{
    fmt::Write as _,
    fs,
    num::NonZero,
    path::{Path, PathBuf},
    thread,
};

use anyhow::{Result, ensure};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, SearcherBuilder, sinks::UTF8};

use super::{code::rel_path, compress::clip, fs as wfs};

/// Matching lines of one file: (path index, [(line number, text)]).
type FileHits = (usize, Vec<(u64, String)>);

/// Searches `base` (workspace or a dependency source dir); paths are shown relative to `root` when inside it.
/// Output is grouped by file: the path once, then `line<TAB>text` rows.
pub fn grep(root: &Path, base: &Path, pattern: &str, glob: Option<&str>, max: usize) -> Result<String> {
    let matcher = RegexMatcherBuilder::new().case_smart(true).build(pattern)?;
    let paths: Vec<PathBuf> = wfs::files(base, glob)?.collect();
    let (hits, total) = scan(&matcher, &paths, max);
    if total == 0 {
        return Ok("no matches".into());
    }
    let (mut body, mut files, mut left) = (String::new(), 0, max);
    for (idx, lines) in &hits {
        if left == 0 {
            break;
        }
        files += 1;
        body.push('\n');
        body.push_str(&rel_path(root, &paths[*idx]));
        for (n, text) in lines.iter().take(left) {
            let _ = write!(body, "\n  {n}\t{text}");
        }
        left = left.saturating_sub(lines.len());
    }
    let count = if total > max { format!("{max}/{total}") } else { total.to_string() };
    let mut out = format!("matches {count} in {files} files{body}");
    if total > max {
        out.push_str("\n(refine pattern or glob)");
    }
    Ok(out)
}

/// Contiguous slices of `paths` per worker: the first `max` lines of each slice are enough to render the first
/// `max` overall, so memory stays bounded however many lines match. Returns hits in path order and the total count.
fn scan(matcher: &RegexMatcher, paths: &[PathBuf], max: usize) -> (Vec<FileHits>, usize) {
    let workers = thread::available_parallelism().map_or(4, NonZero::get).min(16).min(paths.len() / 32 + 1);
    let chunk = paths.len().div_ceil(workers).max(1);
    thread::scope(|s| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .enumerate()
            .map(|(k, part)| s.spawn(move || scan_part(matcher, part, k * chunk, max)))
            .collect();
        let mut hits = Vec::new();
        let mut total = 0;
        for h in handles {
            let (part, count) = h.join().unwrap_or_default();
            hits.extend(part);
            total += count;
        }
        (hits, total)
    })
}

fn scan_part(matcher: &RegexMatcher, part: &[PathBuf], offset: usize, max: usize) -> (Vec<FileHits>, usize) {
    let mut searcher = SearcherBuilder::new().binary_detection(BinaryDetection::quit(0)).line_number(true).build();
    let (mut hits, mut total, mut kept) = (Vec::new(), 0usize, 0usize);
    for (i, path) in part.iter().enumerate() {
        let mut lines = Vec::new();
        let _ = searcher.search_path(
            matcher,
            path,
            UTF8(|line_no, line| {
                total += 1;
                if kept < max {
                    kept += 1;
                    lines.push((line_no, clip(line.trim(), 200)));
                }
                Ok(true)
            }),
        );
        if !lines.is_empty() {
            hits.push((offset + i, lines));
        }
    }
    (hits, total)
}

pub fn self_test() -> Result<String> {
    let dir = std::env::temp_dir().join("omega-selftest-grep");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("probe.txt"), "alpha\nneedle_42\nomega\n")?;
    let out = grep(&dir, &dir, r"needle_\d+", None, 5)?;
    ensure!(out.contains("probe.txt\n  2\tneedle_42"), "résultat inattendu");
    Ok("grep-searcher + ignore (gitignore)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("omega-grep-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "needle\nhay\nneedle\n").unwrap();
        fs::write(dir.join("b.txt"), "hay\nneedle\n").unwrap();
        dir
    }

    #[test]
    fn grep_groups_hits_by_file() {
        let dir = fixture("group");
        let out = grep(&dir, &dir, "needle", None, 80).unwrap();
        assert_eq!(out, "matches 3 in 2 files\na.txt\n  1\tneedle\n  3\tneedle\nb.txt\n  2\tneedle");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_truncated_grep_says_how_many_it_hid() {
        let dir = fixture("cap");
        let out = grep(&dir, &dir, "needle", None, 2).unwrap();
        assert!(out.starts_with("matches 2/3 in 1 files\na.txt\n"), "{out}");
        assert!(out.ends_with("(refine pattern or glob)"), "{out}");
        let _ = fs::remove_dir_all(&dir);
    }
}
