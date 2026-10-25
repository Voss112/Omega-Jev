//! After edits, run the project's own checker once and hand only the verdict/errors to the model.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use super::{code::rel_path, compress::clip, shell};

const TIMEOUT_SECS: usize = 240;
const MAX_CHARS: usize = 3000;

/// Files that the checker would execute while compiling (build scripts, manifests that add them or dependencies,
/// cargo configuration that names a wrapper or runner, toolchain files that point to another compiler).
pub fn builds(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_lowercase();
    let in_cargo_dir = path.parent().and_then(Path::file_name).is_some_and(|d| d.eq_ignore_ascii_case(".cargo"));
    matches!(
        name.as_str(),
        "build.rs"
            | "cargo.toml"
            | "package.json"
            | "tsconfig.json"
            | "setup.py"
            | "pyproject.toml"
            | "go.mod"
            | "go.work"
            | "cmakelists.txt"
            | "rust-toolchain"
            | "rust-toolchain.toml"
    ) || (in_cargo_dir && matches!(name.as_str(), "config" | "config.toml"))
}

/// `None` when no checker applies to the changed files.
pub async fn check(root: &Path, changed: &[PathBuf]) -> Option<String> {
    let exts: BTreeSet<&str> = changed.iter().filter_map(|p| p.extension()?.to_str()).collect();
    let has = |e: &[&str]| e.iter().any(|x| exts.contains(x));
    let marker = |f: &str| root.join(f).is_file();
    let mut checks: Vec<(&str, String)> = Vec::new();
    if has(&["rs"]) && marker("Cargo.toml") {
        checks.push(("cargo check", "cargo check --quiet --message-format=short".into()));
    }
    if has(&["ts", "tsx", "mts", "cts"]) && marker("tsconfig.json") {
        checks.push(("tsc", "npx --no-install tsc --noEmit --pretty false".into()));
    }
    if has(&["go"]) && marker("go.mod") {
        checks.push(("go vet", "go vet ./...".into()));
    }
    let py: Vec<String> = changed
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "py") && p.is_file())
        .map(|p| rel_path(root, p))
        .filter(|r| !r.contains('\''))
        .map(|r| format!("'{r}'"))
        .collect();
    if !py.is_empty() {
        checks.push(("py_compile", format!("python -m py_compile {}", py.join(" "))));
    }
    if checks.is_empty() {
        return None;
    }
    let mut report = String::from("[diagnostics]");
    for (name, cmd) in checks {
        let verdict = match shell::execute(root, &cmd, TIMEOUT_SECS).await {
            Ok(out) if out.starts_with("exit 0\n") => format!("{name}: OK"),
            Ok(out) => format!("{name}: FAILED\n{}", clip(&out, MAX_CHARS)),
            Err(e) => format!("{name}: not run ({e:#})"),
        };
        report.push('\n');
        report.push_str(&verdict);
    }
    Some(report)
}
