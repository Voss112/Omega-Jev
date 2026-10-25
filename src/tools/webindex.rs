//! Local web: every fetched page is chunked and kept on disk; BM25 search costs 0 network and 0 tokens.
//! Loaded on first use, indexed once per change (inverted index), bounded in pages and bytes.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Page {
    pub url: String,
    pub title: String,
    pub fetched: DateTime<Utc>,
    pub chunks: Vec<String>,
}

pub struct Hit {
    pub score: f32,
    pub url: String,
    pub title: String,
    pub text: String,
}

pub struct WebIndex {
    file: PathBuf,
    state: OnceLock<RwLock<State>>,
}

#[derive(Default)]
struct State {
    pages: Vec<Page>,
    /// Built on the first search after a change; `None` = stale.
    index: Option<Indexed>,
}

/// Index over every chunk of every page, plus where each chunk lives.
struct Indexed {
    index: Index,
    refs: Vec<(u32, u32)>,
}

/// Inverted index: term → (doc, term frequency). Queries touch only the postings of their own terms.
struct Index {
    postings: HashMap<Box<str>, Vec<(u32, u16)>>,
    lens: Vec<u32>,
    avg: f32,
}

const MAX_PAGES: usize = 1000;
/// Text kept in memory (and on disk) at most.
const MAX_BYTES: usize = 48 << 20;

impl WebIndex {
    /// Cheap: the file is read on first use, off the UI thread in practice.
    pub fn open(dir: &Path) -> Self {
        Self { file: dir.join("pages.jsonl"), state: OnceLock::new() }
    }

    fn state(&self) -> &RwLock<State> {
        self.state.get_or_init(|| RwLock::new(State { pages: load(&self.file), index: None }))
    }

    fn read(&self) -> RwLockReadGuard<'_, State> {
        self.state().read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, State> {
        self.state().write().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn len(&self) -> usize {
        self.read().pages.len()
    }

    pub fn get(&self, url: &str, max_age_days: i64) -> Option<Page> {
        let fresh = |p: &&Page| Utc::now() - p.fetched < chrono::Duration::days(max_age_days);
        self.read().pages.iter().filter(fresh).find(|p| p.url == url).cloned()
    }

    pub fn put(&self, page: Page) -> Result<()> {
        let line = serde_json::to_string(&page)?;
        let mut state = self.write();
        state.index = None;
        let pages = &mut state.pages;
        pages.retain(|p| p.url != page.url);
        pages.push(page);
        // A superseded copy stays in the file until the next load or eviction rewrites it.
        if evict(pages, MAX_PAGES, MAX_BYTES) {
            return write_all(&self.file, pages);
        }
        writeln!(OpenOptions::new().create(true).append(true).open(&self.file)?, "{line}")?;
        Ok(())
    }

    pub fn search(&self, query: &str, k: usize) -> Vec<Hit> {
        {
            let state = self.read();
            if let Some(indexed) = &state.index {
                return hits(&state.pages, indexed, query, k);
            }
        }
        let mut state = self.write();
        let indexed = state.index.take().unwrap_or_else(|| Indexed::build(&state.pages));
        let found = hits(&state.pages, &indexed, query, k);
        state.index = Some(indexed);
        found
    }
}

impl Indexed {
    fn build(pages: &[Page]) -> Self {
        let refs: Vec<(u32, u32)> = pages
            .iter()
            .enumerate()
            .flat_map(|(p, page)| (0..page.chunks.len()).map(move |c| (p as u32, c as u32)))
            .collect();
        let index = Index::build(pages.iter().flat_map(|p| p.chunks.iter().map(String::as_str)));
        Self { index, refs }
    }
}

impl Index {
    fn build<'a>(docs: impl Iterator<Item = &'a str>) -> Self {
        let mut postings: HashMap<Box<str>, Vec<(u32, u16)>> = HashMap::new();
        let mut lens = Vec::new();
        let mut tf: HashMap<String, u16> = HashMap::new();
        for (doc, text) in docs.enumerate() {
            let mut len = 0u32;
            for t in tokens(text) {
                len += 1;
                let f = tf.entry(t).or_default();
                *f = f.saturating_add(1);
            }
            for (t, f) in tf.drain() {
                match postings.get_mut(t.as_str()) {
                    Some(list) => list.push((doc as u32, f)),
                    None => {
                        postings.insert(t.into_boxed_str(), vec![(doc as u32, f)]);
                    }
                }
            }
            lens.push(len);
        }
        let total: u64 = lens.iter().map(|&l| u64::from(l)).sum();
        let avg = (total as f32 / lens.len().max(1) as f32).max(1.0);
        Self { postings, lens, avg }
    }

    /// Okapi BM25 (k1 = 1.2, b = 0.75). Returns (doc index, score), best first, ties by index.
    fn rank(&self, query: &str, k: usize) -> Vec<(usize, f32)> {
        let terms: HashSet<String> = tokens(query).collect();
        let n = self.lens.len() as f32;
        let mut scores: HashMap<u32, f32> = HashMap::new();
        for t in &terms {
            let Some(list) = self.postings.get(t.as_str()) else { continue };
            let df = list.len() as f32;
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            for &(doc, f) in list {
                let f = f32::from(f);
                let norm = 1.2 * (0.25 + 0.75 * self.lens[doc as usize] as f32 / self.avg);
                *scores.entry(doc).or_default() += idf * f * 2.2 / (f + norm);
            }
        }
        let mut scored: Vec<(usize, f32)> =
            scores.into_iter().filter(|(_, s)| *s > 0.0).map(|(d, s)| (d as usize, s)).collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.truncate(k);
        scored
    }
}

fn hits(pages: &[Page], indexed: &Indexed, query: &str, k: usize) -> Vec<Hit> {
    indexed
        .index
        .rank(query, k)
        .into_iter()
        .map(|(doc, score)| {
            let (p, c) = indexed.refs[doc];
            let page = &pages[p as usize];
            Hit { score, url: page.url.clone(), title: page.title.clone(), text: page.chunks[c as usize].clone() }
        })
        .collect()
}

/// Newest copy of each URL, within the caps; the file is rewritten when anything was dropped.
fn load(file: &Path) -> Vec<Page> {
    let mut pages: Vec<Page> = fs::read_to_string(file)
        .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default();
    let loaded = pages.len();
    let mut seen = HashSet::new();
    pages.reverse();
    pages.retain(|p| seen.insert(p.url.clone()));
    pages.reverse();
    if evict(&mut pages, MAX_PAGES, MAX_BYTES) || pages.len() < loaded {
        let _ = write_all(file, &pages);
    }
    pages
}

fn page_bytes(p: &Page) -> usize {
    p.url.len() + p.title.len() + p.chunks.iter().map(String::len).sum::<usize>()
}

/// Past a cap, drops the oldest pages down to 90 % of it (so the file is not rewritten on every fetch).
fn evict(pages: &mut Vec<Page>, max_pages: usize, max_bytes: usize) -> bool {
    let mut bytes: usize = pages.iter().map(page_bytes).sum();
    if pages.len() <= max_pages && bytes <= max_bytes {
        return false;
    }
    let (keep_pages, keep_bytes) = (max_pages * 9 / 10, max_bytes / 10 * 9);
    let mut drop = 0;
    while drop < pages.len() && (pages.len() - drop > keep_pages || bytes > keep_bytes) {
        bytes -= page_bytes(&pages[drop]);
        drop += 1;
    }
    pages.drain(..drop);
    true
}

/// Rewrites the whole file (drops superseded duplicates and evicted pages).
fn write_all(file: &Path, pages: &[Page]) -> Result<()> {
    let lines: Vec<String> = pages.iter().filter_map(|p| serde_json::to_string(p).ok()).collect();
    // Atomic: a crash mid-rewrite must not leave a truncated cache behind.
    crate::infra::store::write_private(file, (lines.join("\n") + "\n").as_bytes())
}

fn tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2).map(str::to_lowercase)
}

/// One-shot Okapi BM25 over `docs`. Returns (doc index, score), best first.
pub fn bm25(query: &str, docs: &[&str], k: usize) -> Vec<(usize, f32)> {
    if docs.is_empty() {
        return Vec::new();
    }
    Index::build(docs.iter().copied()).rank(query, k)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-index implementation, kept as the ranking reference.
    fn reference(query: &str, docs: &[&str], k: usize) -> Vec<(usize, f32)> {
        let terms: HashSet<String> = tokens(query).collect();
        let freqs: Vec<(usize, HashMap<String, f32>)> = docs
            .iter()
            .map(|d| {
                let mut tf: HashMap<String, f32> = HashMap::new();
                let mut len = 0;
                for t in tokens(d) {
                    len += 1;
                    if terms.contains(&t) {
                        *tf.entry(t).or_default() += 1.0;
                    }
                }
                (len, tf)
            })
            .collect();
        let n = docs.len() as f32;
        let avg = (freqs.iter().map(|f| f.0).sum::<usize>() as f32 / n).max(1.0);
        let idf: HashMap<&str, f32> = terms
            .iter()
            .map(|t| {
                let df = freqs.iter().filter(|f| f.1.contains_key(t)).count() as f32;
                (t.as_str(), ((n - df + 0.5) / (df + 0.5) + 1.0).ln())
            })
            .collect();
        let mut scored: Vec<(usize, f32)> = freqs
            .iter()
            .enumerate()
            .map(|(i, (len, tf))| {
                let norm = 1.2 * (0.25 + 0.75 * *len as f32 / avg);
                (i, tf.iter().map(|(t, f)| idf[t.as_str()] * f * 2.2 / (f + norm)).sum::<f32>())
            })
            .filter(|(_, s)| *s > 0.0)
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(k);
        scored
    }

    fn page(url: &str, text: &str) -> Page {
        Page { url: url.into(), title: String::new(), fetched: Utc::now(), chunks: vec![text.into()] }
    }

    #[test]
    fn indexed_search_ranks_like_plain_bm25() {
        let docs = [
            "Rust async runtimes: tokio drives futures to completion.",
            "Python asyncio and the event loop; async def everywhere.",
            "Rust ownership and borrowing rules, no async here, rust rust.",
            "Unrelated gardening notes.",
        ];
        let got = bm25("rust async", &docs, 10);
        let want = reference("rust async", &docs, 10);
        assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), want.iter().map(|w| w.0).collect::<Vec<_>>());
        for (g, w) in got.iter().zip(&want) {
            assert!((g.1 - w.1).abs() < 1e-4, "{g:?} vs {w:?}");
        }
    }

    #[test]
    fn the_index_evicts_oldest_pages_past_the_byte_cap() {
        let mut pages = vec![page("a", "111111"), page("b", "222222"), page("c", "333333")];
        assert!(evict(&mut pages, 100, 10));
        assert_eq!(pages.iter().map(|p| p.url.as_str()).collect::<Vec<_>>(), ["c"]);
        assert!(!evict(&mut pages, 100, 10), "under the cap nothing moves");
    }

    #[test]
    fn a_new_page_is_searchable_right_away() {
        let dir = std::env::temp_dir().join(format!("omega-webindex-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let index = WebIndex::open(&dir);
        index.put(page("https://a.test/", "tokio runtime")).unwrap();
        assert_eq!(index.search("tokio", 3).len(), 1);
        index.put(page("https://b.test/", "tokio scheduler internals")).unwrap();
        assert_eq!(index.search("tokio", 3).len(), 2, "a put invalidates the index");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "timing, run with --release --nocapture"]
    fn docs_search_timing() {
        let words = ["rust", "tokio", "async", "egui", "serde", "parser", "token", "cache", "index", "query"];
        let dir = std::env::temp_dir().join(format!("omega-webindex-timing-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let index = WebIndex::open(&dir);
        let pages: Vec<Page> = (0..1000)
            .map(|p| Page {
                url: format!("https://x.test/{p}"),
                title: String::new(),
                fetched: Utc::now(),
                chunks: (0..80).map(|c| (0..60).map(|w| words[(p + c + w) % 10]).collect::<Vec<_>>().join(" ")).collect(),
            })
            .collect();
        index.state().write().unwrap().pages = pages;
        let first = std::time::Instant::now();
        index.search("tokio cache", 6);
        let built = first.elapsed();
        let again = std::time::Instant::now();
        index.search("serde parser", 6);
        let next = again.elapsed();
        let all: Vec<String> = index.read().pages.iter().flat_map(|p| p.chunks.clone()).collect();
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
        let old = std::time::Instant::now();
        reference("serde parser", &refs, 6);
        println!("docs_search: ancien {:?}, build+1re {built:?}, suivante {next:?}", old.elapsed());
        let _ = fs::remove_dir_all(&dir);
    }
}
