//! RTK-style filters for the commands agents run most: only what a model acts on survives.
//! Errors and exit codes always pass; a command nobody recognises falls back to the generic filter.

use std::{collections::HashMap, fmt::Write as _, sync::LazyLock};

use regex::Regex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Cargo,
    GitStatus,
    GitDiff,
    Pytest,
    NpmInstall,
}

/// rustc diagnostic head, long (`warning: msg`) or short (`src/a.rs:1:2: warning: msg`) format.
static DIAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\S+:\d+:\d+: )?(warning|error)(?:\[\w+\])?: (.+)$").expect("diag regex"));
static TICKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`]*`").expect("ticks regex"));
static WARN_NOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*= note: `#\[warn\(.+\)\]`.*on by default").expect("note regex"));
static SEPARATORS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&&|\|\||[;|\n]").expect("separator regex"));
static PYTEST_SECTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^=+ (FAILURES|ERRORS) =+$").expect("pytest section regex"));
static PYTEST_SUMMARY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^=+ .*\b(passed|failed|errors?|skipped|no tests ran)\b.* =+$").expect("pytest summary regex")
});

/// Identical warnings beyond this many are counted, not shown.
const KEEP_PER_WARNING: usize = 3;

/// `None` when no specialised filter applies to `cmd`.
pub fn specialize(cmd: &str, text: &str) -> Option<String> {
    match kind(cmd)? {
        Kind::Cargo => Some(cargo(text)),
        Kind::GitStatus => Some(keep_lines(text, |l| !l.trim_start().starts_with("(use \"git "))),
        Kind::GitDiff => Some(git_diff(text)),
        Kind::Pytest => pytest(text),
        Kind::NpmInstall => npm_install(text),
    }
}

/// The last recognised command of the line (`cd x; cargo test 2>&1 | Select-String …` → cargo).
fn kind(cmd: &str) -> Option<Kind> {
    let segments: Vec<&str> = SEPARATORS.split(cmd).collect();
    segments.into_iter().rev().find_map(kind_of)
}

fn kind_of(segment: &str) -> Option<Kind> {
    let owned: Vec<String> = segment
        .split_whitespace()
        .map(|w| w.trim_matches(['"', '\'', '&', '(', ')']).to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    let words: Vec<&str> = owned.iter().map(|w| w.strip_suffix(".exe").unwrap_or(w)).collect();
    let (program, rest) = words.split_first()?;
    let program = program.rsplit(['/', '\\']).next().unwrap_or(program);
    // Positional words only; `git -C dir` / `git -c key=value` options carry a value of their own.
    let mut args: Vec<&str> = Vec::new();
    let mut words_left = rest.iter().copied();
    while let Some(w) = words_left.next() {
        if matches!(w, "-c" | "-C") {
            words_left.next();
        } else if !w.starts_with(['-', '+']) {
            args.push(w);
        }
    }
    let module_pytest = rest.windows(2).any(|w| w == ["-m", "pytest"]);
    match (program, args.first().copied()) {
        ("cargo", Some("build" | "b" | "check" | "c" | "clippy" | "test" | "t" | "run" | "r" | "nextest")) => Some(Kind::Cargo),
        ("git", Some("status")) => Some(Kind::GitStatus),
        ("git", Some("diff" | "show")) => Some(Kind::GitDiff),
        ("pytest" | "py.test", _) => Some(Kind::Pytest),
        ("python" | "python3" | "py", _) if module_pytest => Some(Kind::Pytest),
        ("uv" | "poetry" | "pipenv", Some("run")) if args.get(1) == Some(&"pytest") => Some(Kind::Pytest),
        ("npm" | "pnpm" | "bun", Some("install" | "i" | "ci" | "add")) | ("yarn", None | Some("install" | "add")) => {
            Some(Kind::NpmInstall)
        }
        _ => None,
    }
}

fn keep_lines(text: &str, keep: impl Fn(&str) -> bool) -> String {
    text.lines().filter(|l| keep(l)).collect::<Vec<_>>().join("\n")
}

/// Errors in full; each kind of warning shown three times at most, the rest counted.
fn cargo(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        if let Some(c) = DIAG.captures(line) {
            skipping = false;
            if &c[2] == "warning" && !c[3].contains(" generated ") {
                let key = format!("warning: {}", TICKS.replace_all(&c[3], "`…`"));
                let n = counts.entry(key.clone()).or_insert_with(|| {
                    order.push(key);
                    0
                });
                *n += 1;
                if *n > KEEP_PER_WARNING {
                    // Short format: one line per diagnostic, nothing follows it.
                    skipping = c.get(1).is_none();
                    continue;
                }
            }
        } else if skipping {
            // A blank line closes the skipped diagnostic block.
            skipping = !line.trim().is_empty();
            continue;
        }
        if !WARN_NOTE.is_match(line) {
            out.push(line);
        }
    }
    let mut s = out.join("\n");
    for key in order {
        let n = counts[&key];
        if n > KEEP_PER_WARNING {
            let _ = write!(s, "\n… {} more: {key}", n - KEEP_PER_WARNING);
        }
    }
    s
}

/// Hunks and `+++ b/path` stay; `diff --git`, `index` and the `--- a/path` twin go.
fn git_diff(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let twin = line.starts_with("--- ") && lines.get(i + 1).is_some_and(|n| n.starts_with("+++ b/"));
        if !(line.starts_with("diff --git ") || line.starts_with("index ") || twin) {
            out.push(*line);
        }
    }
    out.join("\n")
}

/// Failures and errors from their section on; a green run is its summary line.
fn pytest(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    if let Some(i) = lines.iter().position(|l| PYTEST_SECTION.is_match(l)) {
        return Some(lines[i..].join("\n"));
    }
    lines.iter().rev().find(|l| PYTEST_SUMMARY.is_match(l)).map(|l| (*l).to_owned())
}

/// Errors and the final tallies; progress, funding and deprecation chatter go.
fn npm_install(text: &str) -> Option<String> {
    const KEEP: &[&str] = &["err!", "error", "added ", "removed ", "changed ", "up to date", "vulnerabilit", "audited "];
    let out = keep_lines(text, |l| {
        let l = l.to_lowercase();
        KEEP.iter().any(|k| l.contains(k))
    });
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unused(n: usize) -> String {
        format!(
            "warning: unused variable: `x{n}`\n --> src/main.rs:{n}:9\n  |\n{n} |     let x{n} = 1;\n  |         ^^ help: prefix it\n  |\n  = note: `#[warn(unused_variables)]` on by default\n\n"
        )
    }

    #[test]
    fn cargo_warnings_are_grouped() {
        let log = (1..=12).map(unused).collect::<String>() + "warning: `demo` (bin \"demo\") generated 12 warnings\n";
        let out = specialize("cargo build", &log).unwrap();
        assert_eq!(out.lines().filter(|l| l.starts_with("warning: unused variable")).count(), 3, "{out}");
        assert!(out.contains("… 9 more: warning: unused variable"), "{out}");
        assert!(!out.contains("#[warn("), "{out}");
        assert!(out.contains("generated 12 warnings"), "{out}");
        assert!(!out.contains("let x7 = 1"), "skipped blocks drop their body: {out}");
    }

    #[test]
    fn a_failing_cargo_build_keeps_every_error() {
        let errors = ["E0308]: mismatched types", "E0425]: cannot find value `y`", "E0599]: no method `z`", "E0382]: use of moved value"];
        let log: String = errors.iter().map(|e| format!("error[{e}\n --> src/lib.rs:3:5\n  |\n3 |     bad\n  |     ^^^\n\n")).collect();
        let out = specialize("cd crate && cargo check --all-targets", &(log + "error: could not compile `demo`\n")).unwrap();
        for e in errors {
            assert!(out.contains(e), "{e} missing: {out}");
        }
        assert_eq!(out.matches("3 |     bad").count(), 4);
        assert!(out.contains("could not compile"));
    }

    #[test]
    fn short_format_warnings_are_grouped_line_by_line() {
        let log: String = (1..=5).map(|n| format!("src/a.rs:{n}:1: warning: unused import: `m{n}`\n")).collect();
        let out = specialize("cargo check --message-format=short", &(log + "    Finished `dev` profile\n")).unwrap();
        assert_eq!(out.matches("unused import").count(), 4, "3 shown + 1 tally: {out}");
        assert!(out.contains("Finished"), "{out}");
    }

    #[test]
    fn git_status_hints_are_dropped() {
        let log = "On branch main\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n  (use \"git restore <file>...\" to discard changes)\n\tmodified:   src/main.rs\n";
        let out = specialize("git status", log).unwrap();
        assert!(!out.contains("(use \"git"), "{out}");
        assert!(out.contains("modified:   src/main.rs"));
    }

    #[test]
    fn git_diff_keeps_hunks_and_one_header() {
        let log = "diff --git a/x b/x\nindex 1a2b3c4..5d6e7f8 100644\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-old\n+new\n context\n";
        let out = specialize("git diff", log).unwrap();
        assert_eq!(out, "+++ b/x\n@@ -1,2 +1,2 @@\n-old\n+new\n context");
    }

    #[test]
    fn pytest_keeps_failures_only() {
        let red = "collected 3 items\n\ntests/test_a.py ..F\n\n=================================== FAILURES ===================================\n___ test_c ___\n    assert 1 == 2\nE   assert 1 == 2\n=========================== short test summary info ============================\nFAILED tests/test_a.py::test_c\n========================= 1 failed, 2 passed in 0.12s ==========================\n";
        let out = specialize("python -m pytest -q", red).unwrap();
        assert!(out.starts_with("=================================== FAILURES"), "{out}");
        assert!(out.contains("E   assert 1 == 2") && out.contains("1 failed, 2 passed"));
        let green = "collected 2 items\n\ntests/test_a.py ..                                         [100%]\n\n============================== 2 passed in 0.03s ===============================\n";
        assert_eq!(specialize("pytest", green).unwrap(), "============================== 2 passed in 0.03s ===============================");
    }

    #[test]
    fn npm_install_keeps_errors_and_summary() {
        let log = "npm warn deprecated inflight@1.0.6: leaks memory\n\nadded 214 packages, and audited 215 packages in 9s\n\n38 packages are looking for funding\n  run `npm fund` for details\n\nfound 0 vulnerabilities\nnpm ERR! code ERESOLVE\n";
        let out = specialize("npm install", log).unwrap();
        assert_eq!(out, "added 214 packages, and audited 215 packages in 9s\nfound 0 vulnerabilities\nnpm ERR! code ERESOLVE");
    }

    #[test]
    fn unknown_command_falls_back_to_generic() {
        assert_eq!(specialize("echo hi", "hi"), None);
        assert_eq!(specialize("echo \"cargo test\"", "cargo test"), None);
    }

    #[test]
    fn the_last_recognised_command_of_a_pipeline_wins() {
        assert_eq!(kind("cargo test 2>&1 | Select-String -Pattern \"^error|test result\""), Some(Kind::Cargo));
        assert_eq!(kind("Set-Location app; .\\venv\\Scripts\\python.exe -m pytest tests"), Some(Kind::Pytest));
        assert_eq!(kind("git -c core.pager=cat diff --stat"), Some(Kind::GitDiff));
    }
}
