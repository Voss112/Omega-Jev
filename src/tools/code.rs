//! Tree-sitter code intelligence (Serena-like): outline, find, surgical symbol replacement.

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, PoisonError},
    thread,
    time::SystemTime,
};

use anyhow::{Context, Result, bail, ensure};
use tree_sitter::{Language, Node, Parser, Tree};

use super::{change::Change, compress::clip, fs as wfs, guard};

pub struct Symbol {
    pub name: String,
    pub kind: &'static str,
    pub sig: String,
    pub line: usize,
    pub end_line: usize,
    pub start: usize,
    pub end: usize,
    pub depth: usize,
    pub parent: Option<String>,
    pub container: bool,
}

pub struct Parsed {
    pub rel: String,
    pub src: String,
    pub symbols: Vec<Symbol>,
    /// Byte ranges of identifier tokens, in source order (the syntax tree itself is not kept).
    pub idents: Vec<(usize, usize)>,
}

/// Parsed workspace files keyed by path, invalidated by mtime, pruned to the last indexed workspace.
type Cache = HashMap<PathBuf, (SystemTime, Arc<Parsed>)>;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);

/// Definition node kinds across all grammars → short label.
const DEFS: &[(&str, &str)] = &[
    ("function_item", "fn"),
    ("function_signature_item", "fn"),
    ("struct_item", "struct"),
    ("enum_item", "enum"),
    ("trait_item", "trait"),
    ("impl_item", "impl"),
    ("mod_item", "mod"),
    ("type_item", "type"),
    ("const_item", "const"),
    ("static_item", "static"),
    ("macro_definition", "macro"),
    ("function_definition", "fn"),
    ("class_definition", "class"),
    ("function_declaration", "fn"),
    ("generator_function_declaration", "fn"),
    ("method_definition", "method"),
    ("class_declaration", "class"),
    ("abstract_class_declaration", "class"),
    ("interface_declaration", "interface"),
    ("type_alias_declaration", "type"),
    ("enum_declaration", "enum"),
    ("method_declaration", "method"),
    ("type_spec", "type"),
    ("struct_specifier", "struct"),
    ("class_specifier", "class"),
    ("namespace_definition", "namespace"),
    ("enum_specifier", "enum"),
    ("constructor_declaration", "ctor"),
    ("record_declaration", "record"),
    ("namespace_declaration", "namespace"),
    ("struct_declaration", "struct"),
];

/// Definitions whose children are listed too (methods inside classes/impls).
const CONTAINERS: &[&str] = &[
    "impl_item",
    "trait_item",
    "mod_item",
    "class_definition",
    "class_declaration",
    "abstract_class_declaration",
    "interface_declaration",
    "class_specifier",
    "struct_specifier",
    "namespace_definition",
    "namespace_declaration",
    "record_declaration",
    "struct_declaration",
];

const MAX_FILES: usize = 4000;

pub fn language(path: &Path) -> Option<Language> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "py" | "pyi" => tree_sitter_python::LANGUAGE.into(),
        "js" | "jsx" | "mjs" | "cjs" => tree_sitter_javascript::LANGUAGE.into(),
        "ts" | "mts" | "cts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "c" | "h" => tree_sitter_c::LANGUAGE.into(),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => tree_sitter_cpp::LANGUAGE.into(),
        "java" => tree_sitter_java::LANGUAGE.into(),
        "cs" => tree_sitter_c_sharp::LANGUAGE.into(),
        "sh" | "bash" => tree_sitter_bash::LANGUAGE.into(),
        _ => return None,
    })
}

pub fn parse(path: &Path, src: &str) -> Option<(Tree, Vec<Symbol>)> {
    let mut parser = Parser::new();
    parser.set_language(&language(path)?).ok()?;
    let tree = parser.parse(src, None)?;
    let mut symbols = Vec::new();
    collect(tree.root_node(), src.as_bytes(), 0, None, &mut symbols);
    Some((tree, symbols))
}

fn collect(node: Node, src: &[u8], depth: usize, parent: Option<&str>, out: &mut Vec<Symbol>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let Some((kind, name)) = def_kind(child).zip(name_of(child, src)) else {
            collect(child, src, depth, parent, out);
            continue;
        };
        let container = CONTAINERS.contains(&child.kind());
        out.push(Symbol {
            sig: signature(child, src),
            name: name.clone(),
            kind,
            line: child.start_position().row + 1,
            end_line: child.end_position().row + 1,
            start: child.start_byte(),
            end: child.end_byte(),
            depth,
            parent: parent.map(str::to_owned),
            container,
        });
        if container {
            collect(child, src, depth + 1, Some(&name), out);
        }
    }
}

fn def_kind(node: Node) -> Option<&'static str> {
    let kind = node.kind();
    if kind == "variable_declarator" {
        let value = node.child_by_field_name("value")?.kind();
        return matches!(value, "arrow_function" | "function_expression" | "function").then_some("fn");
    }
    let label = DEFS.iter().find(|(k, _)| *k == kind)?.1;
    if kind.ends_with("_specifier") && node.child_by_field_name("body").is_none() {
        return None;
    }
    Some(label)
}

fn name_of(node: Node, src: &[u8]) -> Option<String> {
    let text = |n: Node| n.utf8_text(src).ok().map(str::to_owned);
    if let Some(n) = node.child_by_field_name("name") {
        return text(n);
    }
    if node.kind() == "impl_item" {
        let ty = text(node.child_by_field_name("type")?)?;
        return Some(ty.split('<').next().unwrap_or(&ty).trim().to_owned());
    }
    let mut decl = node.child_by_field_name("declarator")?;
    while let Some(inner) = decl.child_by_field_name("declarator") {
        decl = inner;
    }
    text(decl)
}

/// Header up to the body, whitespace-collapsed.
fn signature(node: Node, src: &[u8]) -> String {
    let start = node.start_byte();
    let end = node.child_by_field_name("body").map_or_else(
        || src[start..node.end_byte()].iter().position(|&b| b == b'\n').map_or(node.end_byte(), |p| start + p),
        |b| b.start_byte(),
    );
    let raw = String::from_utf8_lossy(&src[start..end.max(start)]);
    clip(&raw.split_whitespace().collect::<Vec<_>>().join(" "), 200)
}

pub fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

fn render(symbols: &[Symbol]) -> String {
    symbols.iter().map(|s| format!("{}L{} {}", "  ".repeat(s.depth), s.line, s.sig)).collect::<Vec<_>>().join("\n")
}

pub fn outline(root: &Path, rel: &str) -> Result<String> {
    let target = guard::resolve_read(root, rel)?;
    if target.is_file() {
        let src = wfs::read_text(&target)?;
        let (_, symbols) = parse(&target, &src).context("unsupported language; use read_file")?;
        return Ok(render(&symbols));
    }
    let mut out = String::new();
    for path in wfs::files(&target, None)?.filter(|p| language(p).is_some()).take(120) {
        let Some(parsed) = parse_file(root, &path) else { continue };
        let _ = writeln!(out, "## {}\n{}", parsed.rel, render(&parsed.symbols));
        if out.len() > 14_000 {
            out.push_str("… truncated: outline a subdirectory");
            break;
        }
    }
    Ok(if out.is_empty() { "(no supported source files)".into() } else { out })
}

pub fn parsed(rel: String, path: &Path, src: String) -> Option<Parsed> {
    let (tree, symbols) = parse(path, &src)?;
    let idents = identifiers(&tree);
    Some(Parsed { rel, src, symbols, idents })
}

fn parse_file(root: &Path, path: &Path) -> Option<Parsed> {
    parsed(rel_path(root, path), path, wfs::read_text(path).ok()?)
}

/// Leaf identifier tokens (pre-order traversal = source order).
fn identifiers(tree: &Tree) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if node.child_count() == 0 && node.kind().ends_with("identifier") {
            out.push((node.start_byte(), node.end_byte()));
        }
        if cursor.goto_first_child() || cursor.goto_next_sibling() {
            continue;
        }
        loop {
            if !cursor.goto_parent() {
                return out;
            }
            if cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Whole workspace, parsed in parallel; unchanged files come from the cache.
pub fn index(root: &Path) -> Vec<Arc<Parsed>> {
    let Ok(files) = wfs::files(root, None) else { return Vec::new() };
    let paths: Vec<PathBuf> = files.filter(|p| language(p).is_some()).take(MAX_FILES).collect();
    let cached: HashMap<PathBuf, Arc<Parsed>> = {
        let cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        paths
            .iter()
            .filter_map(|p| {
                let (time, parsed) = cache.get(p)?;
                (Some(*time) == mtime(p) && parsed.rel == rel_path(root, p)).then(|| (p.clone(), parsed.clone()))
            })
            .collect()
    };
    let stale: Vec<&PathBuf> = paths.iter().filter(|p| !cached.contains_key(*p)).collect();
    let workers = thread::available_parallelism().map_or(4, std::num::NonZero::get).min(16);
    let chunk = stale.len().div_ceil(workers).max(1);
    let fresh: Vec<(PathBuf, SystemTime, Arc<Parsed>)> = thread::scope(|s| {
        let handles: Vec<_> = stale
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .filter_map(|p| Some(((*p).clone(), mtime(p)?, Arc::new(parse_file(root, p)?))))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    for (path, time, parsed) in fresh {
        cache.insert(path, (time, parsed));
    }
    let keep: HashSet<&PathBuf> = paths.iter().collect();
    cache.retain(|p, _| keep.contains(p));
    paths.iter().filter_map(|p| cache.get(p).map(|(_, parsed)| parsed.clone())).collect()
}

/// `Type::method` / `Class.method` → (Some("Type"), "method").
pub fn split_name(name: &str) -> (Option<&str>, &str) {
    match name.rsplit_once("::").or_else(|| name.rsplit_once('.')) {
        Some((q, base)) => (Some(q.rsplit([':', '.']).next().unwrap_or(q)), base),
        None => (None, name),
    }
}

pub fn matches(s: &Symbol, qual: Option<&str>, base: &str) -> bool {
    let name_ok = s.name == base || s.name.ends_with(&format!("::{base}"));
    name_ok && qual.is_none_or(|q| s.parent.as_deref() == Some(q) || s.name.starts_with(&format!("{q}::")))
}

pub fn find(root: &Path, name: &str, body: bool) -> Result<String> {
    const LIMIT: usize = 20;
    let (qual, base) = split_name(name);
    let files = index(root);
    let hits: Vec<(&Parsed, &Symbol)> = files
        .iter()
        .map(Arc::as_ref)
        .flat_map(|f| f.symbols.iter().filter(|s| matches(s, qual, base)).map(move |s| (f, s)))
        .collect();
    if hits.is_empty() {
        let needle = base.to_lowercase();
        let mut similar: Vec<&str> = files
            .iter()
            .flat_map(|f| &f.symbols)
            .map(|s| s.name.as_str())
            .filter(|n| n.to_lowercase().contains(&needle))
            .collect();
        similar.sort_unstable();
        similar.dedup();
        similar.truncate(12);
        bail!("symbol `{name}` not found. Similar: {}", if similar.is_empty() { "none".into() } else { similar.join(", ") });
    }
    let mut out = String::new();
    for (f, s) in hits.iter().take(LIMIT) {
        let _ = writeln!(out, "{}:{}-{} {} {}", f.rel, s.line, s.end_line, s.kind, s.sig);
        if body {
            out.push_str(&wfs::numbered(&f.src, s.line, s.end_line.min(s.line + 250)));
        }
    }
    if hits.len() > LIMIT {
        let _ = writeln!(out, "… {} more matches; qualify the name", hits.len() - LIMIT);
    }
    Ok(out)
}

pub fn plan_replace(root: &Path, rel: &str, name: &str, code: &str) -> Result<Change> {
    let path = guard::resolve_writable(root, rel)?;
    let src = wfs::read_text(&path)?;
    let (tree, symbols) = parse(&path, &src).context("unsupported language; use edit_file")?;
    let (qual, base) = split_name(name);
    let found: Vec<&Symbol> = symbols.iter().filter(|s| matches(s, qual, base)).collect();
    let target = match found.as_slice() {
        [one] => *one,
        [] => bail!("symbol `{name}` not found in {rel}"),
        many => {
            let lines: Vec<String> = many.iter().map(|s| s.line.to_string()).collect();
            bail!("`{name}` is ambiguous (lines {}); qualify it", lines.join(", "))
        }
    };
    let code = code.trim().replace("\r\n", "\n");
    let code = if src.contains("\r\n") { code.replace('\n', "\r\n") } else { code };
    let after = format!("{}{}{}", &src[..target.start], code, &src[target.end..]);
    let (new_tree, _) = parse(&path, &after).context("reparse failed")?;
    ensure!(tree.root_node().has_error() || !new_tree.root_node().has_error(), "new code has syntax errors; file left unchanged");
    let action = format!("replaced {name} in {rel} (L{}-{})", target.line, target.end_line);
    Ok(Change { action, rel: rel.to_owned(), path, before: Some(src), after })
}

pub fn self_test() -> Result<String> {
    const SAMPLES: [(&str, &str); 11] = [
        ("a.rs", "fn main() {}"),
        ("a.py", "def f():\n    pass\n"),
        ("a.js", "function f() {}"),
        ("a.ts", "function f(): void {}"),
        ("a.tsx", "function F() { return <div/> }"),
        ("a.go", "package m\nfunc F() {}\n"),
        ("a.c", "int f(void) { return 0; }"),
        ("a.cpp", "int f() { return 0; }"),
        ("a.java", "class A { void f() {} }"),
        ("a.cs", "class A { void F() {} }"),
        ("a.sh", "f() { echo hi; }"),
    ];
    let ok = SAMPLES.iter().filter(|(p, s)| parse(Path::new(p), s).is_some_and(|(_, syms)| !syms.is_empty())).count();
    ensure!(ok == SAMPLES.len(), "{ok}/{} grammaires opérationnelles", SAMPLES.len());
    Ok(format!("{ok}/{} langages", SAMPLES.len()))
}
