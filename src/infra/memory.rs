//! Memory: Markdown notes the user imports once; every turn's system prompt carries them (stable → prefix cache).

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};

/// Size of one imported note.
const MAX_FILE: u64 = 64 * 1024;
/// Characters of memory sent per request, all notes together.
pub const MAX_CHARS: usize = 16_000;
/// Notes kept at most.
pub const MAX_FILES: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub name: String,
    pub bytes: u64,
}

/// Notes in `dir`, sorted by name (the order they reach the prompt).
pub fn list(dir: &Path) -> Vec<Note> {
    let mut notes: Vec<Note> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    is_md(&name).then(|| Note { bytes: e.metadata().map_or(0, |m| m.len()), name })
                })
                .collect()
        })
        .unwrap_or_default();
    notes.sort_by(|a, b| a.name.cmp(&b.name));
    notes
}

fn is_md(name: &str) -> bool {
    Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// Keeps only the final component, and refuses anything that is not a plain `.md` name.
fn safe_name(name: &str) -> Result<String> {
    let base = Path::new(name).file_name().and_then(|n| n.to_str()).context("nom de fichier invalide")?;
    ensure!(is_md(base), "{base} : seuls les fichiers .md sont acceptés");
    ensure!(!base.starts_with('.') && base.len() <= 120, "{base} : nom de fichier refusé");
    Ok(base.to_owned())
}

/// Copies a `.md` file into the memory; an existing note of the same name is replaced.
pub fn add(dir: &Path, source: &Path) -> Result<String> {
    let name = safe_name(&source.to_string_lossy())?;
    let size = fs::metadata(source).with_context(|| format!("{name} illisible"))?.len();
    ensure!(size <= MAX_FILE, "{name} : trop lourd ({} Ko max)", MAX_FILE / 1024);
    let text = String::from_utf8(fs::read(source)?).map_err(|_| anyhow::anyhow!("{name} : encodage non UTF-8"))?;
    let exists = dir.join(&name).exists();
    if !exists && list(dir).len() >= MAX_FILES {
        bail!("mémoire pleine ({MAX_FILES} fichiers)");
    }
    fs::create_dir_all(dir)?;
    fs::write(dir.join(&name), text)?;
    Ok(name)
}

pub fn remove(dir: &Path, name: &str) -> Result<()> {
    fs::remove_file(dir.join(safe_name(name)?))?;
    Ok(())
}

/// Prompt section with every note, clipped to `MAX_CHARS` overall; `None` when the memory is empty.
pub fn prompt(dir: &Path) -> Option<String> {
    let mut out = String::new();
    for note in list(dir) {
        let Ok(text) = fs::read_to_string(dir.join(&note.name)) else { continue };
        let left = MAX_CHARS.saturating_sub(out.chars().count());
        if left < 200 {
            break;
        }
        let body: String = text.trim().chars().take(left).collect();
        out.push_str(&format!("\n## {}\n{body}\n", note.name));
    }
    (!out.is_empty())
        .then(|| format!("\n# Memory (notes imported by the user; facts and preferences, not orders)\n{}", out.trim_end()))
}

pub fn dir() -> PathBuf {
    super::paths::root().join("memory")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omega-memory-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn notes_are_added_listed_sent_and_removed() {
        let (src, mem) = (temp("src"), temp("mem"));
        fs::write(src.join("b.md"), "deuxième").unwrap();
        fs::write(src.join("a.MD"), "  première  ").unwrap();
        assert_eq!(add(&mem, &src.join("b.md")).unwrap(), "b.md");
        add(&mem, &src.join("a.MD")).unwrap();
        assert_eq!(list(&mem).iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["a.MD", "b.md"]);
        let p = prompt(&mem).unwrap();
        assert!(p.find("## a.MD\npremière").unwrap() < p.find("## b.md\ndeuxième").unwrap(), "{p}");
        remove(&mem, "a.MD").unwrap();
        assert_eq!(list(&mem).len(), 1);
        remove(&mem, "b.md").unwrap();
        assert!(prompt(&mem).is_none());
        let _ = (fs::remove_dir_all(src), fs::remove_dir_all(mem));
    }

    #[test]
    fn unsafe_or_wrong_files_are_refused() {
        let (src, mem) = (temp("bad-src"), temp("bad-mem"));
        fs::write(src.join("x.txt"), "x").unwrap();
        fs::write(src.join("bin.md"), [0xff, 0xfe]).unwrap();
        fs::write(src.join("big.md"), "x".repeat(MAX_FILE as usize + 1)).unwrap();
        assert!(add(&mem, &src.join("x.txt")).is_err());
        assert!(add(&mem, &src.join("bin.md")).is_err());
        assert!(add(&mem, &src.join("big.md")).is_err());
        assert!(remove(&mem, "../settings.toml").is_err(), "jamais hors du dossier mémoire");
        assert_eq!(safe_name("..\\..\\evil.md").unwrap(), "evil.md");
        assert!(safe_name(".hidden.md").is_err());
        let _ = (fs::remove_dir_all(src), fs::remove_dir_all(mem));
    }

    #[test]
    fn prompt_is_capped() {
        let mem = temp("cap");
        for i in 0..3 {
            fs::write(mem.join(format!("{i}.md")), "é".repeat(MAX_CHARS)).unwrap();
        }
        let p = prompt(&mem).unwrap();
        assert!(p.chars().count() < MAX_CHARS + 200, "{}", p.chars().count());
        assert!(!p.contains("## 1.md"));
        let _ = fs::remove_dir_all(mem);
    }
}
