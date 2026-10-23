//! Impact analysis (code-review-graph-like): symbol reference graph on petgraph, reverse BFS.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
};

use anyhow::{Result, ensure};
use petgraph::{
    Direction,
    graph::{DiGraph, NodeIndex},
};

use super::{
    code::{self, Parsed},
    compress::toon,
};

const MAX_DEPTH: usize = 3;
const MAX_ROWS: usize = 80;

pub fn blast_radius(root: &Path, name: &str) -> Result<String> {
    let files: Vec<_> = code::index(root);
    analyze(&files.iter().map(AsRef::as_ref).collect::<Vec<_>>(), name)
}

fn short(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

fn analyze(files: &[&Parsed], name: &str) -> Result<String> {
    let mut graph: DiGraph<(usize, usize), ()> = DiGraph::new();
    let mut by_name: HashMap<&str, Vec<NodeIndex>> = HashMap::new();
    let nodes: Vec<Vec<NodeIndex>> = files
        .iter()
        .enumerate()
        .map(|(fi, f)| {
            f.symbols
                .iter()
                .enumerate()
                .map(|(si, s)| {
                    let n = graph.add_node((fi, si));
                    by_name.entry(short(&s.name)).or_default().push(n);
                    n
                })
                .collect()
        })
        .collect();

    for (fi, f) in files.iter().enumerate() {
        let idents = identifiers(f);
        for (si, s) in f.symbols.iter().enumerate().filter(|(_, s)| !s.container) {
            let lo = idents.partition_point(|(b, _)| *b < s.start);
            let hi = idents.partition_point(|(b, _)| *b < s.end);
            let mut seen = HashSet::new();
            for (_, id) in &idents[lo..hi] {
                if *id == short(&s.name) || !seen.insert(*id) {
                    continue;
                }
                for &target in by_name.get(id).into_iter().flatten() {
                    graph.add_edge(nodes[fi][si], target, ());
                }
            }
        }
    }

    let (qual, base) = code::split_name(name);
    let roots: Vec<NodeIndex> = by_name
        .get(base)
        .into_iter()
        .flatten()
        .copied()
        .filter(|n| {
            let (fi, si) = graph[*n];
            code::matches(&files[fi].symbols[si], qual, base)
        })
        .collect();
    ensure!(!roots.is_empty(), "symbol `{name}` not found");

    let mut depth: HashMap<NodeIndex, usize> = roots.iter().map(|r| (*r, 0)).collect();
    let mut queue: VecDeque<NodeIndex> = roots.iter().copied().collect();
    while let Some(n) = queue.pop_front() {
        let d = depth[&n];
        if d == MAX_DEPTH {
            continue;
        }
        for caller in graph.neighbors_directed(n, Direction::Incoming) {
            if let std::collections::hash_map::Entry::Vacant(e) = depth.entry(caller) {
                e.insert(d + 1);
                queue.push_back(caller);
            }
        }
    }

    let mut rows: Vec<(usize, Vec<String>)> = depth
        .iter()
        .filter(|(_, d)| **d > 0)
        .map(|(n, d)| {
            let (fi, si) = graph[*n];
            let s = &files[fi].symbols[si];
            (*d, vec![d.to_string(), files[fi].rel.clone(), s.line.to_string(), s.sig.clone()])
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1[1].cmp(&b.1[1])));
    let file_count = rows.iter().map(|r| r.1[1].as_str()).collect::<HashSet<_>>().len();
    if rows.is_empty() {
        return Ok(format!("{} definition(s) of `{name}`, no dependents found (name-based analysis)", roots.len()));
    }
    let total = rows.len();
    let rows: Vec<Vec<String>> = rows.into_iter().take(MAX_ROWS).map(|r| r.1).collect();
    Ok(format!(
        "{} definition(s), {total} dependent symbol(s) in {file_count} file(s), name-based\n{}",
        roots.len(),
        toon("dependents", &["depth", "file", "line", "symbol"], &rows)
    ))
}

/// Identifier tokens with their byte offset, in source order.
fn identifiers(f: &Parsed) -> Vec<(usize, &str)> {
    f.idents.iter().filter_map(|&(s, e)| Some((s, f.src.get(s..e)?))).collect()
}

pub fn self_test() -> Result<String> {
    let src = "fn leaf() {}\nfn mid() { leaf(); }\nfn top() { mid(); }\n".to_owned();
    let probe = code::parsed("probe.rs".into(), Path::new("probe.rs"), src).ok_or_else(|| anyhow::anyhow!("parse"))?;
    let out = analyze(&[&probe], "leaf")?;
    ensure!(out.contains("fn mid()") && out.contains("fn top()"), "graphe incomplet");
    Ok("petgraph, profondeur 3".into())
}
