//! Per-turn file snapshots: every file the agent touches is copied before its first change → one-click undo.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::infra::paths;

#[derive(Default, Serialize, Deserialize)]
struct Manifest {
    entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    /// Backup file name inside the checkpoint dir; `None` = file did not exist (undo deletes it).
    backup: Option<String>,
}

pub struct Checkpoint {
    dir: PathBuf,
}

impl Checkpoint {
    pub fn new(conv: Uuid, turn: i64) -> Self {
        Self { dir: paths::checkpoints().join(conv.to_string()).join(turn.to_string()) }
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join("manifest.json")
    }

    fn load(&self) -> Manifest {
        fs::read(self.manifest_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn exists(&self) -> bool {
        self.manifest_path().is_file()
    }

    /// Saves the original bytes of `path` the first time it is touched in this turn.
    pub fn record(&self, path: &Path) -> Result<()> {
        let mut manifest = self.load();
        if manifest.entries.iter().any(|e| e.path == path) {
            return Ok(());
        }
        fs::create_dir_all(&self.dir).context("checkpoint dir")?;
        let backup = if path.is_file() {
            let name = format!("{}.orig", manifest.entries.len());
            fs::copy(path, self.dir.join(&name)).context("checkpoint copy")?;
            Some(name)
        } else {
            None
        };
        manifest.entries.push(Entry { path: path.to_owned(), backup });
        fs::write(self.manifest_path(), serde_json::to_vec(&manifest)?)?;
        Ok(())
    }

    /// Restores every recorded file (deleting files the turn created), then drops the checkpoint.
    /// A missing (pruned) checkpoint restores nothing.
    pub fn restore(&self) -> Result<usize> {
        if !self.exists() {
            return Ok(0);
        }
        let manifest = self.load();
        for entry in manifest.entries.iter().rev() {
            match &entry.backup {
                Some(name) if backup_name(name) => {
                    fs::copy(self.dir.join(name), &entry.path).with_context(|| format!("restore {}", entry.path.display()))?;
                }
                Some(_) => bail!("checkpoint corrompu : nom de sauvegarde inattendu"),
                None => {
                    let _ = fs::remove_file(&entry.path);
                }
            }
        }
        fs::remove_dir_all(&self.dir)?;
        Ok(manifest.entries.len())
    }
}

/// Backup files are named `<index>.orig`; anything else in a manifest is refused.
fn backup_name(name: &str) -> bool {
    name.strip_suffix(".orig").is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Deletes turn checkpoints older than `max_age` (disk hygiene, run at startup).
pub fn prune(max_age: Duration) {
    let Ok(convs) = fs::read_dir(paths::checkpoints()) else { return };
    for turn in convs.flatten().filter_map(|c| fs::read_dir(c.path()).ok()).flatten().flatten() {
        let old = turn.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > max_age);
        if old {
            let _ = fs::remove_dir_all(turn.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_brings_back_edited_and_removes_created_files() {
        let dir = std::env::temp_dir().join("omega-test-checkpoint");
        fs::create_dir_all(&dir).unwrap();
        let (edited, created) = (dir.join("a.txt"), dir.join("new.txt"));
        fs::write(&edited, "v1").unwrap();
        let _ = fs::remove_file(&created);
        let cp = Checkpoint { dir: dir.join("cp") };
        cp.record(&edited).unwrap();
        cp.record(&created).unwrap();
        fs::write(&edited, "v2").unwrap();
        fs::write(&created, "x").unwrap();
        assert_eq!(cp.restore().unwrap(), 2);
        assert_eq!(fs::read_to_string(&edited).unwrap(), "v1");
        assert!(!created.exists() && !cp.exists());
    }

    #[test]
    fn a_tampered_manifest_cannot_copy_from_outside_the_checkpoint() {
        let dir = std::env::temp_dir().join("omega-test-checkpoint-traversal");
        let _ = fs::remove_dir_all(&dir);
        let cp = Checkpoint { dir: dir.join("cp") };
        fs::create_dir_all(&cp.dir).unwrap();
        fs::write(dir.join("victim.txt"), "secret").unwrap();
        let manifest = format!(r#"{{"entries":[{{"path":"{}","backup":"../../victim.txt"}}]}}"#, dir.join("out.txt").display());
        fs::write(cp.manifest_path(), manifest.replace('\\', "\\\\")).unwrap();
        assert!(cp.restore().is_err(), "un nom de sauvegarde hors dossier doit être refusé");
        assert!(!dir.join("out.txt").exists());
    }
}
