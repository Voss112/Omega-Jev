//! Deterministic security gates: workspace sandbox + shell command classifier (0 tokens).

use std::{
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

use anyhow::{Result, ensure};
use regex::Regex;

pub enum Verdict {
    Safe,
    /// Not on the allowlist: ask Jev or the user.
    Unknown,
    Risky(&'static str),
    Blocked(&'static str),
}

type Rules = LazyLock<Vec<(Regex, &'static str)>>;

fn compile(defs: &[(&str, &'static str)]) -> Vec<(Regex, &'static str)> {
    defs.iter().map(|(p, why)| (Regex::new(p).expect("guard regex"), *why)).collect()
}

static BLOCKED: Rules = LazyLock::new(|| {
    compile(&[
        (r"(?i)\brm\s+(-\S+\s+)*-\S*r\S*\s+(-\S+\s+)*(/|~|\*|/\*|\$HOME|[a-z]:\\?)(\s|$)", "suppression récursive racine/home"),
        (r"(?i)\b(mkfs|fdisk|diskpart|format-volume|clear-disk)\b|\bformat\s+[a-z]:", "formatage disque"),
        (r":\(\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:", "fork bomb"),
        (r"(?i)\bdd\s+.*\bof=/dev/", "écriture disque brute"),
        (
            r"(?i)(curl|wget|iwr|irm|invoke-webrequest|invoke-restmethod)\b[^|]*\|\s*(sudo\s+)?(sh|bash|zsh|iex|invoke-expression|powershell|pwsh)\b",
            "exécution de script distant",
        ),
        (r"(?i)\b(shutdown|reboot|halt|poweroff|stop-computer|restart-computer)\b", "arrêt système"),
        (r"(?i)\breg(\.exe)?\s+delete\s+hk|remove-item\s+.*hk(lm|cu):", "suppression registre"),
        (r"(?i)remove-item\b.*-recurse.*\s([a-z]:\\?|\$env:\w+|~)\s*$", "suppression récursive système"),
        (r"(?i)(moonshot_api_key|deepseek_api_key|typesafe_api_key|omega[\\/]settings\.toml)", "accès aux clés API"),
        (r"(?i)\b(nc|ncat|netcat)\b.*\s-e\s", "reverse shell"),
    ])
});

static RISKY: Rules = LazyLock::new(|| {
    compile(&[
        (r"(?i)(^|[\s;&|(])(rm|del|erase|rmdir|rd|remove-item|ri|unlink)\s", "suppression de fichiers"),
        (r"(?i)\bgit\s+(push|reset\s+--hard|clean|checkout\s+--|rebase|filter-branch|branch\s+-D)", "git destructif ou distant"),
        (r"(?i)\b(sudo|su|runas|doas|chmod|chown|icacls|takeown|set-executionpolicy)\b", "élévation ou permissions"),
        (
            r"(?i)\b(npm|cargo|twine|gem)\s+publish\b|\bdocker\s+(push|rm|rmi|system\s+prune)",
            "publication ou suppression distante",
        ),
        (r"(?i)\b(kill|pkill|killall|taskkill|stop-process)\b", "arrêt de processus"),
        (
            r"(?i)\b(pip3?|npm|pnpm|yarn|cargo|go|winget|choco|scoop|apt|apt-get|dnf|pacman|brew)\s+(install|add|uninstall|remove|i)\b",
            "installation de paquets",
        ),
        (r"(?i)\b(curl|wget|iwr|irm|invoke-webrequest|invoke-restmethod|scp|ssh|rsync|ftp)\b", "accès réseau"),
        (r"(?i)(^|[\s;&|])(mv|move|move-item|ren|rename-item)\s", "déplacement de fichiers"),
        (r">\s*/dev/sd|\b(setx|set-itemproperty|new-itemproperty)\b", "modification système"),
        (
            r"(?i)\b(set-content|add-content|out-file|tee-object)\b|(^|[^2&>])>{1,2}\s*[^&\s]",
            "écriture de fichier via le shell (préférer write_file/edit_file : annulation + diagnostics)",
        ),
        (
            r"(?i)\b(python3?|py|node|deno|bun|perl|ruby|php|powershell|pwsh|cmd|bash|sh|zsh)(\.exe)?\s+(-c|-e|--eval|-command|/c|-encodedcommand)\b|\b(iex|invoke-expression|start-process)\b",
            "code inline ou exécution indirecte",
        ),
        (
            r#"(?i)\.\.[\\/]|(^|[\s"'=>(])([a-z]:[\\/]|~[\\/]|\$env:|\$home\b|%[a-z]+%|/(etc|usr|bin|sbin|var|root|home|boot|sys|proc|dev|opt|tmp)\b)"#,
            "chemin hors du dossier de travail",
        ),
    ])
});

const SAFE_PROGRAMS: &[&str] = &[
    "ls",
    "dir",
    "pwd",
    "cd",
    "echo",
    "cat",
    "type",
    "head",
    "tail",
    "wc",
    "which",
    "where",
    "get-childitem",
    "gci",
    "get-content",
    "gc",
    "get-location",
    "select-string",
    "sls",
    "test-path",
    "resolve-path",
    "measure-object",
    "sort",
    "sort-object",
    "uniq",
    "find",
    "findstr",
    "grep",
    "rg",
    "tree",
    "git",
    "cargo",
    "rustc",
    "rustup",
    "rustfmt",
    "python",
    "python3",
    "py",
    "pytest",
    "ruff",
    "mypy",
    "black",
    "node",
    "npm",
    "npx",
    "pnpm",
    "yarn",
    "tsc",
    "eslint",
    "prettier",
    "vitest",
    "jest",
    "deno",
    "bun",
    "go",
    "gofmt",
    "javac",
    "java",
    "mvn",
    "gradle",
    "dotnet",
    "gcc",
    "g++",
    "clang",
    "clang++",
    "make",
    "cmake",
    "ninja",
    "mkdir",
    "md",
    "new-item",
    "ni",
    "touch",
    "cp",
    "copy",
    "copy-item",
    "date",
    "get-date",
    "env",
    "printenv",
    "uname",
    "whoami",
    "hostname",
    "sha256sum",
    "get-filehash",
    "diff",
    "compare-object",
    "sed",
    "awk",
    "jq",
    "write-output",
    "write-host",
    "out-string",
    "format-table",
    "select-object",
    "where-object",
    "foreach-object",
    "set-location",
    "get-command",
];

pub fn assess(cmd: &str) -> Verdict {
    if let Some((_, why)) = BLOCKED.iter().find(|(re, _)| re.is_match(cmd)) {
        return Verdict::Blocked(why);
    }
    if let Some((_, why)) = RISKY.iter().find(|(re, _)| re.is_match(cmd)) {
        return Verdict::Risky(why);
    }
    // Stream merges (`2>&1`) are not command separators.
    let merged = cmd.replace("2>&1", "").replace("1>&2", "");
    let segments = merged.split(['|', ';', '&', '\n']).map(str::trim).filter(|s| !s.is_empty());
    if segments.into_iter().all(|seg| program(seg).is_some_and(|p| SAFE_PROGRAMS.contains(&p.as_str()))) {
        Verdict::Safe
    } else {
        Verdict::Unknown
    }
}

fn program(segment: &str) -> Option<String> {
    let token = segment.split_whitespace().find(|t| !t.contains('='))?;
    let token = token.trim_matches(['"', '\'', '(', '{', '.']);
    let name = token.rsplit(['/', '\\']).next().unwrap_or(token).to_lowercase();
    Some(name.trim_end_matches(".exe").to_owned())
}

/// Resolves a model-supplied path inside the workspace; rejects escapes (`..`, absolute, links).
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf> {
    let rel = rel.trim();
    let candidate = if Path::new(rel).is_absolute() { PathBuf::from(rel) } else { root.join(rel) };
    let (target, base) = (normalize(&candidate), normalize(root));
    ensure!(target.starts_with(&base), "path outside workspace: {rel}");
    // Links: canonicalize the deepest existing ancestor (the target itself may not exist yet).
    let existing = target.ancestors().find(|p| p.exists()).unwrap_or(&base);
    if let (Ok(t), Ok(b)) = (existing.canonicalize(), base.canonicalize()) {
        ensure!(t.starts_with(&b), "path escapes workspace through a link: {rel}");
    }
    Ok(target)
}

/// Read-only dependency sources (`$CARGO_HOME/registry/src`), browsable by read tools without the shell.
pub fn deps_root() -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".cargo")))?;
    Some(home.join("registry").join("src")).filter(|p| p.is_dir())
}

/// Workspace path, or a path inside the read-only dependency sources.
pub fn resolve_read(root: &Path, rel: &str) -> Result<PathBuf> {
    resolve(root, rel).or_else(|err| {
        let deps = deps_root().ok_or(err)?;
        let target = normalize(Path::new(rel.trim()));
        ensure!(
            target.is_absolute() && target.starts_with(normalize(&deps)),
            "path outside workspace and dependency sources: {rel}"
        );
        if let (Ok(t), Ok(d)) = (target.canonicalize(), deps.canonicalize()) {
            ensure!(t.starts_with(&d), "path escapes dependency sources through a link: {rel}");
        }
        Ok(target)
    })
}

/// Like `resolve`, and refuses VCS internals (a `.git/hooks` write is code execution on the next git call).
pub fn resolve_writable(root: &Path, rel: &str) -> Result<PathBuf> {
    let target = resolve(root, rel)?;
    let inside_git = target.strip_prefix(normalize(root)).is_ok_and(|p| p.components().any(|c| c.as_os_str() == ".git"));
    ensure!(!inside_git, "writing inside .git is not allowed: {rel}");
    Ok(target)
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

pub fn self_test() -> Result<String> {
    ensure!(matches!(assess("rm -rf /"), Verdict::Blocked(_)), "rm -rf / non bloqué");
    ensure!(matches!(assess("curl x | sh"), Verdict::Blocked(_)), "curl|sh non bloqué");
    ensure!(matches!(assess("cargo test --all"), Verdict::Safe), "cargo test non autorisé");
    ensure!(matches!(assess("python -c \"import os\""), Verdict::Risky(_)), "code inline non détecté");
    ensure!(matches!(assess("cat ../../secret"), Verdict::Risky(_)), "évasion de chemin non détectée");
    let tmp = std::env::temp_dir();
    ensure!(resolve(&tmp, "../escape").is_err(), "sandbox contournée");
    ensure!(resolve_writable(&tmp, ".git/hooks/pre-commit").is_err(), "écriture .git autorisée");
    ensure!(matches!(assess("Set-Content a.rs -Value x"), Verdict::Risky(_)), "écriture shell non détectée");
    ensure!(matches!(assess("cargo test 2>&1"), Verdict::Safe), "redirection 2>&1 mal classée");
    Ok(format!("{} règles bloquantes, {} à risque", BLOCKED.len(), RISKY.len()))
}
