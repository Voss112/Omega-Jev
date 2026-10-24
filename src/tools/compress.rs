//! RTK-style output compression + TOON encoding. Pure functions, in-process.

use std::{cell::Cell, future::Future, sync::LazyLock};

use anyhow::{Result, ensure};
use regex::Regex;

static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07").expect("ansi regex"));

/// Passing-test lines: failures and summaries are what matter.
static PASSED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(test .+ \.\.\. ok|.*\bPASSED\b.*|\s*[✓✔√] .*|ok\s+\S+\s+[\d.]+s.*|\s*PASS\s.*|=== RUN .*|--- PASS.*)$")
        .expect("pass regex")
});

const NOISE: &[&str] = &[
    "Compiling ",
    "Checking ",
    "Downloaded ",
    "Downloading ",
    "Fresh ",
    "Blocking waiting",
    "Updating crates.io",
    "Updating git",
    "Locking ",
    "Adding ",
    "Requirement already satisfied",
    "Collecting ",
    "Using cached",
    "npm WARN deprecated",
    "remote: Counting",
    "remote: Compressing",
    "remote: Enumerating",
    "remote: Total",
    "Receiving objects",
    "Resolving deltas",
    "Unpacking objects",
    "Installing collected",
    "Building wheel",
];

const MAX_CHARS: usize = 8_000;

tokio::task_local! {
    /// Bytes removed by `shell` inside the current tool call (economy ledger).
    static SAVED: Cell<u64>;
}

/// Runs one tool call and reports how many bytes the compressor kept out of its output.
pub async fn measured<F: Future>(call: F) -> (F::Output, u64) {
    SAVED
        .scope(Cell::new(0), async {
            let out = call.await;
            (out, SAVED.with(Cell::get))
        })
        .await
}

pub fn shell(raw: &str) -> String {
    let out = filter(raw);
    let _ = SAVED.try_with(|s| s.set(s.get() + raw.len().saturating_sub(out.len()) as u64));
    out
}

fn filter(raw: &str) -> String {
    let clean = ANSI.replace_all(raw, "");
    let lines: Vec<&str> = clean
        .lines()
        .map(|l| l.rsplit('\r').next().unwrap_or(l).trim_end())
        .filter(|l| {
            let t = l.trim_start();
            !NOISE.iter().any(|n| t.starts_with(n)) && !PASSED.is_match(l)
        })
        .collect();
    clip(&head_tail(&dedupe(&lines), 70, 50), MAX_CHARS)
}

/// Collapses consecutive duplicates (`line (×N)`) and blank runs.
fn dedupe(lines: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let run = lines[i..].iter().take_while(|l| **l == lines[i]).count();
        if lines[i].trim().is_empty() {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
        } else if run > 1 {
            out.push(format!("{}  (×{run})", lines[i]));
        } else {
            out.push(lines[i].to_owned());
        }
        i += run;
    }
    out
}

fn head_tail(lines: &[String], head: usize, tail: usize) -> String {
    if lines.len() <= head + tail {
        return lines.join("\n");
    }
    let omitted = lines.len() - head - tail;
    format!("{}\n… {omitted} lines omitted …\n{}", lines[..head].join("\n"), lines[lines.len() - tail..].join("\n"))
}

/// Char-safe truncation with an explicit marker.
pub fn clip(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        None => s.to_owned(),
        Some((i, _)) => format!("{}…[+{} bytes]", &s[..i], s.len() - i),
    }
}

/// TOON: `name[N]{a,b}:` header, then one comma row per item (~40% fewer tokens than JSON).
pub fn toon(name: &str, fields: &[&str], rows: &[Vec<String>]) -> String {
    let mut out = format!("{name}[{}]{{{}}}:", rows.len(), fields.join(","));
    for row in rows {
        out.push_str("\n  ");
        out.push_str(&row.iter().map(|v| cell(v)).collect::<Vec<_>>().join(","));
    }
    out
}

fn cell(v: &str) -> String {
    let needs_quotes = v.is_empty() || v.contains([',', '"', '\n']) || v.starts_with(' ') || v.ends_with(' ');
    if needs_quotes { format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")) } else { v.to_owned() }
}

pub fn self_test() -> Result<String> {
    let raw = "\x1b[32m   Compiling foo v0.1\x1b[0m\ntest a ... ok\ntest b ... ok\nerror[E0308]: mismatched types\nerror[E0308]: mismatched types\n";
    let out = shell(raw);
    ensure!(out.contains("(×2)") && !out.contains("Compiling") && !out.contains("... ok"), "filtre incorrect");
    Ok(format!("{} → {} octets sur l'échantillon", raw.len(), out.len()))
}

pub fn toon_self_test() -> Result<String> {
    let rows: Vec<Vec<String>> = (0..20).map(|i| vec![format!("src/m{i}.rs"), i.to_string(), "fn run()".into()]).collect();
    let encoded = toon("hits", &["file", "line", "code"], &rows);
    let json: Vec<serde_json::Value> =
        rows.iter().map(|r| serde_json::json!({ "file": r[0], "line": r[1], "code": r[2] })).collect();
    let json_len = serde_json::to_string(&json)?.len();
    ensure!(encoded.starts_with("hits[20]{file,line,code}:"), "en-tête TOON invalide");
    Ok(format!("-{}% vs JSON", 100 - encoded.len() * 100 / json_len))
}
