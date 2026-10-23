//! Local web: every fetched page is chunked and kept on disk; BM25 search costs 0 network and 0 tokens.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{PoisonError, RwLock, RwLockReadGuard},
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
    pages: RwLock<Vec<Page>>,
}

const MAX_PAGES: usize = 1000;

impl WebIndex {
    pub fn open(dir: &Path) -> Self {
        let file = dir.join("pages.jsonl");
        let mut pages: Vec<Page> = fs::read_to_string(&file)
            .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
            .unwrap_or_default();
        let loaded = pages.len();
        let mut seen = HashSet::new();
        pages.reverse();
        pages.retain(|p| seen.insert(p.url.clone()));
        pages.reverse();
        let index = Self { file, pages: RwLock::new(pages) };
        if index.len() < loaded {
            let _ = write_all(&index.file, &index.read());
        }
        index
    }

    fn read(&self) -> RwLockReadGuard<'_, Vec<Page>> {
        self.pages.read().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn len(&self) -> usize {
        self.read().len()
    }

    pub fn get(&self, url: &str, max_age_days: i64) -> Option<Page> {
        let fresh = |p: &&Page| Utc::now() - p.fetched < chrono::Duration::days(max_age_days);
        self.read().iter().filter(fresh).find(|p| p.url == url).cloned()
    }

    pub fn put(&self, page: Page) -> Result<()> {
        let line = serde_json::to_string(&page)?;
        let mut pages = self.pages.write().unwrap_or_else(PoisonError::into_inner);
        pages.retain(|p| p.url != page.url);
        pages.push(page);
        if pages.len() > MAX_PAGES {
            let excess = pages.len() - MAX_PAGES;
            pages.drain(..excess);
            return write_all(&self.file, &pages);
        }
        writeln!(OpenOptions::new().create(true).append(true).open(&self.file)?, "{line}")?;
        Ok(())
    }

    pub fn search(&self, query: &str, k: usize) -> Vec<Hit> {
        let pages = self.read();
        let docs: Vec<(&Page, &str)> = pages.iter().flat_map(|p| p.chunks.iter().map(move |c| (p, c.as_str()))).collect();
        let texts: Vec<&str> = docs.iter().map(|d| d.1).collect();
        bm25(query, &texts, k)
            .into_iter()
            .map(|(i, score)| Hit {
                score,
                url: docs[i].0.url.clone(),
                title: docs[i].0.title.clone(),
                text: docs[i].1.to_owned(),
            })
            .collect()
    }
}

/// Rewrites the whole file (drops superseded duplicates and evicted pages).
fn write_all(file: &Path, pages: &[Page]) -> Result<()> {
    let lines: Vec<String> = pages.iter().filter_map(|p| serde_json::to_string(p).ok()).collect();
    fs::write(file, lines.join("\n") + "\n")?;
    Ok(())
}

fn tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2).map(str::to_lowercase)
}

/// Okapi BM25 (k1 = 1.2, b = 0.75). Returns (doc index, score), best first.
pub fn bm25(query: &str, docs: &[&str], k: usize) -> Vec<(usize, f32)> {
    let terms: HashSet<String> = tokens(query).collect();
    if terms.is_empty() || docs.is_empty() {
        return Vec::new();
    }
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
