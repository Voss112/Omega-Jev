//! Compact tool schemas (short descriptions = fewer prompt tokens).

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::{domain::tool::ToolKind, infra::process};

pub fn schemas(set: &BTreeSet<ToolKind>) -> Value {
    Value::Array(set.iter().map(|k| spec(*k)).collect())
}

type Param = (&'static str, &'static str, &'static str, bool);

fn spec(kind: ToolKind) -> Value {
    let shell_desc = format!(
        "Run a {} command in the workspace. Output is compressed. Risky commands need user approval.",
        process::shell_name()
    );
    let (desc, params): (&str, &[Param]) = match kind {
        ToolKind::ListDir => (
            "Directory tree (gitignore-aware).",
            &[("path", "string", "Dir, default .", false), ("depth", "integer", "1-5, default 2", false)],
        ),
        ToolKind::ReadFile => (
            "Read file lines with numbers (default first 200). Prefer ranges.",
            &[
                ("path", "string", "File", true),
                ("start", "integer", "First line, 1-based", false),
                ("end", "integer", "Last line", false),
            ],
        ),
        ToolKind::Outline => (
            "Symbol signatures with line numbers, no bodies. Cheapest way to understand code.",
            &[("path", "string", "File or dir", false)],
        ),
        ToolKind::FindSymbol => (
            "Locate symbol definitions in the workspace.",
            &[("name", "string", "Name, Type::method or Class.method", true), ("body", "boolean", "Include source", false)],
        ),
        ToolKind::Grep => (
            "Regex search, smart case, max 80 hits.",
            &[
                ("pattern", "string", "Regex", true),
                ("glob", "string", "e.g. *.rs", false),
                ("path", "string", "Dir, default . (deps dir allowed)", false),
            ],
        ),
        ToolKind::BlastRadius => {
            ("Transitive callers/users of a symbol (depth 3): impact of changing it.", &[("name", "string", "Symbol name", true)])
        }
        ToolKind::EditFile => (
            "Replace exact text in a file. `old` must be unique unless all=true.",
            &[
                ("path", "string", "File", true),
                ("old", "string", "Exact current text", true),
                ("new", "string", "Replacement", true),
                ("all", "boolean", "Replace all", false),
            ],
        ),
        ToolKind::ReplaceSymbol => (
            "Replace a whole symbol definition (signature+body). Syntax-checked, reverted on error.",
            &[
                ("path", "string", "File", true),
                ("name", "string", "Symbol (Type::method ok)", true),
                ("code", "string", "Full new definition, without preceding doc comments/attributes", true),
            ],
        ),
        ToolKind::WriteFile => {
            ("Create or overwrite a file.", &[("path", "string", "File", true), ("content", "string", "Full content", true)])
        }
        ToolKind::Shell => {
            ("", &[("command", "string", "Command line", true), ("timeout", "integer", "Seconds, default 120", false)])
        }
        ToolKind::SecurityScan => (
            "Static security scan: secrets, injections, unsafe APIs, weak crypto, TLS bypass. Verify findings.",
            &[("path", "string", "File or dir, default .", false)],
        ),
        ToolKind::DocsSearch => (
            "BM25 search over locally cached web pages. Free, instant: try before web_search.",
            &[("query", "string", "Keywords", true)],
        ),
        ToolKind::WebSearch => ("Anonymous web search; results relevance-filtered.", &[("query", "string", "Query", true)]),
        ToolKind::WebFetch => (
            "Fetch a page as compact markdown; with focus returns only the most relevant passages. Cached locally.",
            &[("url", "string", "http(s) URL", true), ("focus", "string", "What you need from the page", false)],
        ),
        ToolKind::CliAnything => (
            "Drive real apps (GIMP, Blender, LibreOffice, Inkscape, OBS, Audacity, Kdenlive, Zotero, Obsidian, draw.io, FreeCAD…) \
             via CLI-Anything agent-native CLIs. Flow: search → info (usage) → install (once, user-approved) → run (JSON output).",
            &[
                ("action", "string", "search | info | install | run", true),
                ("query", "string", "search: app or task keywords", false),
                ("name", "string", "CLI name from search", false),
                ("args", "array", "run: arguments, e.g. [\"project\",\"new\",\"-o\",\"p.json\"]", false),
                ("timeout", "integer", "run: seconds, default 300", false),
            ],
        ),
    };
    let desc = if kind == ToolKind::Shell { shell_desc.as_str() } else { desc };
    let props: Map<String, Value> = params
        .iter()
        .map(|&(n, t, d, _)| {
            let schema = if t == "array" {
                json!({ "type": t, "items": { "type": "string" }, "description": d })
            } else {
                json!({ "type": t, "description": d })
            };
            (n.to_owned(), schema)
        })
        .collect();
    let required: Vec<&str> = params.iter().filter(|p| p.3).map(|p| p.0).collect();
    json!({
        "type": "function",
        "function": {
            "name": kind.name(),
            "description": desc,
            "parameters": { "type": "object", "properties": props, "required": required }
        }
    })
}
