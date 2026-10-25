//! Native tool belt (single binary): replaces Serena, code-review-graph, rtk, lazy-tool, caveman, Semgrep-lite.

pub mod audit;
pub mod change;
pub mod checkpoint;
pub mod clihub;
pub mod code;
pub mod compress;
pub mod computer;
pub mod diagnostics;
pub mod fs;
pub mod graph;
pub mod guard;
pub mod kits;
pub mod livetest;
pub mod reads;
pub mod rtk;
pub mod schema;
pub mod search;
pub mod security;
pub mod shell;
pub mod web;
pub mod webindex;

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    app::{Approval, ApprovalKind, Emitter, Env, Event},
    domain::{chat::ToolCall, tool::ToolKind},
};

pub struct ToolCtx<'a> {
    pub env: &'a Env,
    pub root: PathBuf,
    pub emit: &'a Emitter,
    pub conv: Uuid,
    /// Turn key (checkpoint folder).
    pub turn: i64,
    /// File ranges already on the wire, so an unchanged re-read costs a stub.
    pub reads: &'a reads::Reads,
}

pub struct ToolOutput {
    /// What the model sees.
    pub text: String,
    /// Unified diff for the UI only.
    pub diff: Option<String>,
    /// File written by this call (drives diagnostics).
    pub changed: Option<PathBuf>,
    /// Bytes the RTK compressor removed before the model saw `text`.
    pub rtk: u64,
    /// Screenshots the model sees with the result (vision engines).
    pub images: Vec<crate::domain::chat::Image>,
}

impl From<String> for ToolOutput {
    fn from(text: String) -> Self {
        Self { text, diff: None, changed: None, rtk: 0, images: Vec::new() }
    }
}

/// Never fails: errors become tool output so the model can recover.
pub async fn run(ctx: &ToolCtx<'_>, call: &ToolCall) -> ToolOutput {
    let args: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
    let (result, rtk) = compress::measured(async {
        match ToolKind::parse(&call.name) {
            Some(kind) if ctx.env.settings.disabled_tools.contains(&kind) => {
                Err(anyhow!("tool `{}` disabled by the user", call.name))
            }
            Some(kind) => {
                ctx.reads.ran(kind, &args);
                exec(ctx, kind, Args(&args)).await
            }
            None => Err(anyhow!("unknown tool `{}`", call.name)),
        }
    })
    .await;
    ToolOutput { rtk, ..result.unwrap_or_else(|e| format!("ERROR: {e:#}").into()) }
}

async fn exec(ctx: &ToolCtx<'_>, kind: ToolKind, a: Args<'_>) -> Result<ToolOutput> {
    let root = ctx.root.clone();
    let text = match kind {
        ToolKind::Computer => return computer::run(ctx, a.0).await,
        ToolKind::ListDir => {
            let (path, depth) = (a.opt("path").unwrap_or_else(|| ".".into()), a.num("depth").unwrap_or(2).clamp(1, 5));
            blocking(move || fs::list(&root, &path, depth)).await
        }
        ToolKind::ReadFile => {
            let (path, start, end) = (a.str("path")?, a.num("start"), a.num("end"));
            let key = reads::key(&path, start, end);
            match ctx.reads.unchanged(&root, &key) {
                Some(stub) => Ok(stub),
                None => {
                    let out = blocking(move || fs::read(&root, &path, start, end)).await;
                    if out.is_ok() {
                        ctx.reads.record(key);
                    }
                    out
                }
            }
        }
        ToolKind::Outline => {
            let path = a.opt("path").unwrap_or_else(|| ".".into());
            blocking(move || code::outline(&root, &path)).await
        }
        ToolKind::FindSymbol => {
            let (name, body) = (a.str("name")?, a.flag("body"));
            blocking(move || code::find(&root, &name, body)).await
        }
        ToolKind::Grep => {
            let (pattern, glob, path) = (a.str("pattern")?, a.opt("glob"), a.opt("path").unwrap_or_else(|| ".".into()));
            blocking(move || search::grep(&root, &guard::resolve_read(&root, &path)?, &pattern, glob.as_deref(), 80)).await
        }
        ToolKind::BlastRadius => {
            let name = a.str("name")?;
            blocking(move || graph::blast_radius(&root, &name)).await
        }
        ToolKind::EditFile => {
            let (path, old, new, all, until) = (a.str("path")?, a.str("old")?, a.str("new")?, a.flag("all"), a.opt("until"));
            return change::apply(ctx, blocking(move || fs::plan_edit(&root, &path, &old, &new, all, until.as_deref())).await?).await;
        }
        ToolKind::ReplaceSymbol => {
            let (path, name, code) = (a.str("path")?, a.str("name")?, a.str("code")?);
            return change::apply(ctx, blocking(move || code::plan_replace(&root, &path, &name, &code)).await?).await;
        }
        ToolKind::WriteFile => {
            let (path, content) = (a.str("path")?, a.str("content")?);
            return change::apply(ctx, blocking(move || fs::plan_write(&root, &path, content)).await?).await;
        }
        ToolKind::Shell => shell::run(ctx, &a.str("command")?, a.num("timeout").unwrap_or(120)).await,
        ToolKind::SecurityScan => {
            let path = a.opt("path").unwrap_or_else(|| ".".into());
            blocking(move || security::scan(&root, &path)).await
        }
        ToolKind::DocsSearch => {
            let query = a.str("query")?;
            let web = ctx.env.web.clone();
            blocking(move || Ok(web::docs_search(&web, &query))).await
        }
        ToolKind::WebSearch => web::search(ctx, &a.str("query")?).await,
        ToolKind::WebFetch => web::fetch(ctx, &a.str("url")?, a.opt("focus").as_deref()).await,
        ToolKind::CliAnything => match a.str("action")?.as_str() {
            "search" => clihub::search(ctx, &a.opt("query").unwrap_or_default()).await,
            "info" => clihub::info(ctx, &a.str("name")?).await,
            "install" => clihub::install(ctx, &a.str("name")?).await,
            "run" => clihub::run(ctx, &a.str("name")?, a.list("args"), a.num("timeout").unwrap_or(300)).await,
            other => Err(anyhow!("unknown action `{other}` (search|info|install|run)")),
        },
        ToolKind::AuditKit => {
            let manifest = format!("{} fichiers → {}", audit::FILES.len(), root.join(".omega/skills/security-audit").display());
            let reason = "Installation du skill d'audit Cloudflare (documentation, jamais exécutée)".to_owned();
            if ctx.env.settings.confirm_edits && !approve(ctx, ApprovalKind::Edit, manifest, reason).await {
                bail!("installation refusée");
            }
            blocking(move || audit::install(&root)).await
        }
        ToolKind::Kits => match a.str("action")?.as_str() {
            "list" => Ok(kits::list(a.opt("category").as_deref())),
            "info" => kits::info(&a.str("name")?),
            "install" => kits::install(ctx, &a.str("name")?, a.opt("component").as_deref()).await,
            other => Err(anyhow!("unknown action `{other}` (list|info|install)")),
        },
    };
    text.map(ToolOutput::from)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await.context("tool task crashed")?
}

/// Asks the user in the chat; `false` if refused or the turn is gone. Bypass mode grants everything.
async fn approve(ctx: &ToolCtx<'_>, kind: ApprovalKind, subject: String, reason: String) -> bool {
    if ctx.env.settings.unattended() {
        return true;
    }
    let (reply, rx) = oneshot::channel();
    ctx.emit.send(Event::Approval(Approval { conv: ctx.conv, kind, subject, reason, reply }));
    rx.await.unwrap_or(false)
}

/// Typed accessors over the model's JSON arguments.
#[derive(Clone, Copy)]
struct Args<'a>(&'a Value);

impl Args<'_> {
    fn opt(self, key: &str) -> Option<String> {
        self.0.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
    }

    fn str(self, key: &str) -> Result<String> {
        self.0.get(key).and_then(Value::as_str).map(str::to_owned).with_context(|| format!("missing argument `{key}`"))
    }

    fn num(self, key: &str) -> Option<usize> {
        let v = self.0.get(key)?;
        v.as_u64().or_else(|| v.as_str()?.parse().ok()).map(|n| n as usize)
    }

    /// String array; a plain string is split on whitespace.
    fn list(self, key: &str) -> Vec<String> {
        match self.0.get(key) {
            Some(Value::Array(items)) => items.iter().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned)).collect(),
            Some(Value::String(s)) => s.split_whitespace().map(str::to_owned).collect(),
            _ => Vec::new(),
        }
    }

    fn flag(self, key: &str) -> bool {
        self.0.get(key).is_some_and(|v| v.as_bool() == Some(true) || v.as_str() == Some("true"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_local_tool_passes_its_self_test() {
        let results = [
            ("code", super::code::self_test()),
            ("graph", super::graph::self_test()),
            ("search", super::search::self_test()),
            ("compress", super::compress::self_test()),
            ("toon", super::compress::toon_self_test()),
            ("security", super::security::self_test()),
            ("guard", super::guard::self_test()),
        ];
        for (name, result) in results {
            assert!(result.is_ok(), "{name}: {:?}", result.err());
        }
    }

    #[test]
    fn replace_symbol_rejects_broken_code() {
        let dir = std::env::temp_dir().join("omega-test-replace");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("m.rs"), "fn a() -> u8 { 1 }\nfn b() {}\n").unwrap();
        assert!(super::code::plan_replace(&dir, "m.rs", "a", "fn a() -> u8 { 2 ").is_err());
        let change = super::code::plan_replace(&dir, "m.rs", "a", "fn a() -> u8 { 2 }").unwrap();
        assert!(change.after.contains("{ 2 }") && change.after.contains("fn b()"));
    }

    #[test]
    fn a_non_utf8_file_is_never_edited_in_place() {
        let dir = std::env::temp_dir().join(format!("omega-test-latin1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("l.txt"), b"caf\xe9 au lait\n").unwrap();
        let err = super::fs::plan_edit(&dir, "l.txt", "lait", "miel", false, None).err().unwrap();
        assert!(err.to_string().contains("not UTF-8"), "{err}");
        std::fs::write(dir.join("l.rs"), b"fn a() { let s = \"\xe9\"; }\n").unwrap();
        assert!(super::code::plan_replace(&dir, "l.rs", "a", "fn a() {}").is_err(), "replace_symbol aussi");
        assert!(super::fs::read(&dir, "l.txt", None, None).unwrap().contains("caf"), "la lecture reste possible");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edit_file_replaces_a_block_given_its_first_and_last_lines() {
        let dir = std::env::temp_dir().join(format!("omega-test-until-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("m.rs"), "fn keep() {}\r\nfn old() {\r\n    a();\r\n    b();\r\n}\r\nfn tail() {}\r\n").unwrap();
        let edit = |old: &str, until: Option<&str>| super::fs::plan_edit(&dir, "m.rs", old, "fn new() {}", false, until);
        let change = edit("fn old() {", Some("    b();\n}")).unwrap();
        assert_eq!(change.after, "fn keep() {}\r\nfn new() {}\r\nfn tail() {}\r\n", "bloc remplacé, CRLF respecté");
        assert!(change.action.contains("4 line(s)"), "{}", change.action);
        assert!(edit("fn old() {", Some("absent")).is_err(), "`until` introuvable");
        assert!(edit("fn tail() {}", Some("fn keep()")).is_err(), "`until` doit suivre `old`");
        assert!(edit("fn", Some("}")).is_err(), "`old` ambigu");
        std::fs::write(dir.join("m.rs"), format!("start\n{}end\n", "x\n".repeat(400))).unwrap();
        assert!(edit("start", Some("end")).is_err(), "bloc trop long");
        let _ = std::fs::remove_dir_all(dir);
    }
}
