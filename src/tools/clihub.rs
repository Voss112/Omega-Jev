//! CLI-Anything (HKUDS, Apache-2.0): agent-native CLIs that drive real apps (GIMP, Blender, LibreOffice, OBS…).
//! Native client: registry search → SKILL.md usage → user-approved install → gated run with `--json`.

use std::{
    env,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{LazyLock, OnceLock},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{
    ToolCtx, approve,
    compress::{clip, toon},
    shell,
    webindex::bm25,
};
use crate::{
    app::ApprovalKind,
    infra::{http::read_capped, paths, process},
};

const REPO_RAW: &str = "https://raw.githubusercontent.com/HKUDS/CLI-Anything/main/";
const REGISTRIES: [&str; 2] = ["registry.json", "public_registry.json"];
const HARNESS_GIT: &str = "git+https://github.com/HKUDS/CLI-Anything.git#subdirectory=";
const REFRESH: Duration = Duration::from_hours(24);
const SKILL_MAX: usize = 6000;
const INSTALL_TIMEOUT: usize = 900;

#[derive(Clone, Serialize, Deserialize)]
pub struct Cli {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub requires: Option<String>,
    #[serde(default)]
    pub install_cmd: Option<String>,
    #[serde(default)]
    pub entry_point: Option<String>,
    #[serde(default)]
    pub skill_md: Option<String>,
}

impl Cli {
    fn exe(&self) -> String {
        self.entry_point.clone().unwrap_or_else(|| format!("cli-anything-{}", self.name))
    }

    /// HKUDS harnesses accept the global `--json` flag.
    fn harness(&self) -> bool {
        self.exe().starts_with("cli-anything-")
    }
}

#[derive(Deserialize)]
struct RegistryFile {
    clis: Vec<Cli>,
}

/// Merged official + public registries, cached on disk for a day (works offline from cache).
pub async fn registry(http: &Client) -> Result<Vec<Cli>> {
    let file = paths::clihub().join("registry.json");
    let fresh = fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a < REFRESH);
    if !fresh {
        match download(http).await {
            Ok(clis) => {
                fs::create_dir_all(paths::clihub())?;
                fs::write(&file, serde_json::to_vec(&clis)?)?;
                return Ok(clis);
            }
            Err(e) if !file.is_file() => return Err(e.context("CLI-Anything registry unreachable")),
            Err(_) => {}
        }
    }
    Ok(serde_json::from_slice(&fs::read(&file)?)?)
}

async fn download(http: &Client) -> Result<Vec<Cli>> {
    let mut all: Vec<Cli> = Vec::new();
    for name in REGISTRIES {
        let res = http.get(format!("{REPO_RAW}{name}")).timeout(Duration::from_secs(20)).send().await?.error_for_status()?;
        let file: RegistryFile = serde_json::from_slice(&read_capped(res, 4 << 20).await?)?;
        for cli in file.clis {
            if !all.iter().any(|c| c.name == cli.name) {
                all.push(cli);
            }
        }
    }
    Ok(all)
}

async fn find(ctx: &ToolCtx<'_>, name: &str) -> Result<Cli> {
    let clis = registry(&ctx.env.http).await?;
    let key = name.trim().trim_start_matches("cli-anything-").to_lowercase();
    clis.into_iter()
        .find(|c| c.name.to_lowercase() == key)
        .with_context(|| format!("`{name}` is not in the CLI-Anything registry; use action=search"))
}

pub async fn search(ctx: &ToolCtx<'_>, query: &str) -> Result<String> {
    let clis = registry(&ctx.env.http).await?;
    let docs = documents(&clis);
    let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
    let q = query.to_lowercase();
    let mut picked: Vec<usize> = (0..clis.len()).filter(|&i| !q.is_empty() && clis[i].name.to_lowercase().contains(&q)).collect();
    picked.extend(bm25(query, &refs, 12).into_iter().map(|(i, _)| i).filter(|i| !picked.contains(i)).collect::<Vec<_>>());
    if picked.is_empty() {
        let all: Vec<&str> = clis.iter().map(|c| c.name.as_str()).collect();
        return Ok(format!("no match for `{query}`. Available ({}): {}", all.len(), all.join(", ")));
    }
    let rows: Vec<Vec<String>> = picked
        .iter()
        .take(12)
        .map(|&i| {
            let c = &clis[i];
            let installed = if find_exe(&c.exe()).is_some() { "yes" } else { "no" };
            vec![
                c.name.clone(),
                c.category.clone().unwrap_or_default(),
                installed.into(),
                clip(c.requires.as_deref().unwrap_or(""), 60),
                clip(&c.description, 140),
            ]
        })
        .collect();
    Ok(toon("clis", &["name", "category", "installed", "requires", "description"], &rows))
}

fn documents(clis: &[Cli]) -> Vec<String> {
    clis.iter()
        .map(|c| format!("{} {} {} {}", c.name, c.display_name, c.category.as_deref().unwrap_or(""), c.description))
        .collect()
}

/// Benchmark: the registry loads, a plain-language query finds GIMP and its install is of a safe shape.
pub async fn self_check(http: &Client) -> Result<String> {
    let clis = registry(http).await?;
    let docs = documents(&clis);
    let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
    let found = bm25("raster image editing photo", &refs, 12).into_iter().any(|(i, _)| clis[i].name == "gimp");
    ensure!(found, "la recherche « raster image editing » ne trouve pas GIMP");
    let gimp = clis.iter().find(|c| c.name == "gimp").context("gimp absent du registre")?;
    ensure!(gimp.install_cmd.as_deref().and_then(install_plan).is_some(), "installation GIMP non planifiable sans shell");
    let exe = gimp.exe();
    let installed = tokio::task::spawn_blocking(move || find_exe(&exe).is_some()).await?;
    Ok(format!(
        "{} CLIs · « raster image editing » → gimp · install pip sûre{}",
        clis.len(),
        if installed { " · gimp installé" } else { "" }
    ))
}

pub async fn info(ctx: &ToolCtx<'_>, name: &str) -> Result<String> {
    let cli = find(ctx, name).await?;
    let installed = find_exe(&cli.exe()).map_or_else(|| "not installed (action=install)".to_owned(), |p| p.display().to_string());
    let mut out = format!(
        "{} — {}\nrequires: {}\nexecutable: {} [{installed}]\n",
        cli.display_name,
        cli.description,
        cli.requires.as_deref().unwrap_or("-"),
        cli.exe()
    );
    match skill(&ctx.env.http, &cli).await {
        Some(md) => {
            let _ = write!(out, "\n[usage, untrusted third-party doc]\n{}", clip(&md, SKILL_MAX));
        }
        None => {
            let _ = write!(out, "\nNo SKILL.md: run with args [\"--help\"] to discover commands.");
        }
    }
    Ok(out)
}

/// SKILL.md from the repo (or an https URL), cached per CLI.
async fn skill(http: &Client, cli: &Cli) -> Option<String> {
    let safe_name = cli.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let cache = paths::clihub().join("skills").join(format!("{}.md", cli.name));
    if safe_name && let Ok(text) = fs::read_to_string(&cache) {
        return Some(text);
    }
    let src = cli.skill_md.as_deref()?;
    let url = if src.starts_with("https://") {
        src.to_owned()
    } else if std::path::Path::new(src).extension().is_some_and(|e| e.eq_ignore_ascii_case("md"))
        && !src.contains("..")
        && !src.contains(' ')
    {
        format!("{REPO_RAW}{}", src.trim_start_matches('/'))
    } else {
        return None;
    };
    let res = http.get(url).timeout(Duration::from_secs(20)).send().await.ok()?.error_for_status().ok()?;
    let text = String::from_utf8_lossy(&read_capped(res, 512 << 10).await.ok()?).into_owned();
    if safe_name && fs::create_dir_all(cache.parent()?).is_ok() {
        let _ = fs::write(&cache, &text);
    }
    Some(text)
}

pub async fn install(ctx: &ToolCtx<'_>, name: &str) -> Result<String> {
    let cli = find(ctx, name).await?;
    let raw = cli.install_cmd.clone().context("no install command in the registry")?;
    let (program, args) =
        install_plan(&raw).with_context(|| format!("automatic install not supported; ask the user to run: {raw}"))?;
    let display = format!("{program} {}", args.join(" "));
    let reason = format!(
        "Installer {} : code tiers du registre CLI-Anything ({})",
        cli.display_name,
        cli.requires.as_deref().unwrap_or("-")
    );
    ensure!(approve(ctx, ApprovalKind::Command, display, reason).await, "install refused by the user");
    let mut command = process::command(program);
    command.args(&args);
    let out = shell::run_command(command, &ctx.root, INSTALL_TIMEOUT, "").await?;
    let status = find_exe(&cli.exe())
        .map_or_else(|| format!("{} not found on PATH yet", cli.exe()), |p| format!("ready: {}", p.display()));
    Ok(format!("{out}\n{status}"))
}

/// Only registry commands of a known safe shape are executed, without a shell.
fn install_plan(cmd: &str) -> Option<(&'static str, Vec<String>)> {
    static PACKAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^@?[A-Za-z0-9][A-Za-z0-9._/-]*$").expect("package regex"));
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    match tokens.as_slice() {
        ["pip", "install", spec] if is_safe_pip(spec, &PACKAGE) => {
            Some(("python", ["-m", "pip", "install", "--user", "--upgrade", spec].map(str::to_owned).to_vec()))
        }
        ["npm", "install", "-g", pkg] if PACKAGE.is_match(pkg) => {
            Some((process::npm(), ["install", "-g", pkg].map(str::to_owned).to_vec()))
        }
        _ => None,
    }
}

fn is_safe_pip(spec: &str, package: &Regex) -> bool {
    match spec.strip_prefix(HARNESS_GIT) {
        Some(dir) => !dir.contains("..") && dir.chars().all(|c| c.is_ascii_alphanumeric() || "_-/.".contains(c)),
        None => package.is_match(spec),
    }
}

pub async fn run(ctx: &ToolCtx<'_>, name: &str, mut args: Vec<String>, timeout_secs: usize) -> Result<String> {
    let cli = find(ctx, name).await?;
    let exe = find_exe(&cli.exe()).with_context(|| format!("{} is not installed: call action=install first", cli.exe()))?;
    if cli.harness() && !args.iter().any(|a| a == "--json") {
        args.insert(0, "--json".into());
    }
    shell::gate(ctx, &format!("{} {}", cli.exe(), args.join(" "))).await?;
    let mut command = process::command(&exe);
    command.args(&args);
    shell::run_command(command, &ctx.root, timeout_secs.clamp(1, 1800), "").await
}

/// Executable lookup in PATH plus Python user/global script dirs (where `pip --user` puts entry points).
/// The name comes from a third-party registry: a path (`../x`, `C:\x`) is never looked up.
pub fn find_exe(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\', ':']) || name.contains("..") {
        return None;
    }
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat"] } else { &[""] };
    let path_dirs: Vec<PathBuf> = env::var_os("PATH").map(|p| env::split_paths(&p).collect()).unwrap_or_default();
    path_dirs
        .iter()
        .chain(scripts_dirs())
        .flat_map(|dir| exts.iter().map(move |ext| dir.join(format!("{name}{ext}"))))
        .find(|p| p.is_file())
}

fn scripts_dirs() -> &'static [PathBuf] {
    static DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    DIRS.get_or_init(|| {
        let probe =
            "import os,sysconfig;print(sysconfig.get_path('scripts'));print(sysconfig.get_path('scripts',os.name+'_user'))";
        let mut dirs: Vec<PathBuf> = process::blocking("python")
            .args(["-c", probe])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(|l| PathBuf::from(l.trim())).collect())
            .unwrap_or_default();
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".local").join("bin"));
        }
        dirs
    })
}

/// Health line: registry size, installed CLIs, Python availability.
pub async fn probe(http: &Client) -> Result<String> {
    let clis = registry(http).await?;
    let total = clis.len();
    let (installed, python) = tokio::task::spawn_blocking(move || {
        let installed = clis.iter().filter(|c| find_exe(&c.exe()).is_some()).count();
        (installed, python_version())
    })
    .await?;
    let python = python.context("Python 3.10+ introuvable (requis pour installer les CLIs)")?;
    Ok(format!("{total} CLIs · {installed} installé(s) · {python}"))
}

fn python_version() -> Option<String> {
    let out = process::blocking("python").arg("--version").output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_safe_install_commands_are_planned() {
        let ok = format!("pip install {HARNESS_GIT}gimp/agent-harness");
        assert_eq!(install_plan(&ok).map(|p| p.0), Some("python"));
        assert!(install_plan("npm install -g @larksuite/cli").is_some());
        assert!(install_plan("pip install x; rm -rf /").is_none());
        assert!(install_plan(&format!("pip install {HARNESS_GIT}../evil")).is_none());
        assert!(install_plan("cd sketch/agent-harness && npm install").is_none());
        assert!(install_plan("brew install foo").is_none());
    }

    #[test]
    fn a_registry_entry_point_cannot_name_a_path() {
        for name in ["../../evil", r"C:\Windows\System32\cmd", "sub/tool", ""] {
            assert!(find_exe(name).is_none(), "{name}");
        }
    }

    /// Live check against GitHub: `cargo test -- --ignored clihub`.
    #[tokio::test]
    #[ignore = "network"]
    async fn clihub_live_registry_and_skill() {
        let http = crate::infra::http::client("").unwrap();
        let clis = registry(&http).await.unwrap();
        let gimp = clis.iter().find(|c| c.name == "gimp").expect("gimp in registry");
        assert!(clis.len() > 50 && install_plan(gimp.install_cmd.as_deref().unwrap()).is_some());
        assert!(skill(&http, gimp).await.unwrap().contains("cli-anything-gimp"));
        assert!(!scripts_dirs().is_empty(), "python scripts dirs");
    }
}
