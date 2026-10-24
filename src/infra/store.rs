use std::{fs, io::Write as _, path::Path};

use anyhow::Result;
use uuid::Uuid;

use super::{paths, secret};
use crate::domain::{chat::Conversation, settings::Settings};

/// API keys are decrypted in memory; legacy plaintext keys are re-saved encrypted.
/// Subscription tokens never live here: they are sealed in `credentials/*.json`.
/// The notice reports a settings file that could not be read (kept aside, never overwritten).
pub fn load_settings() -> (Settings, Option<String>) {
    let path = paths::settings();
    let Ok(text) = fs::read_to_string(&path) else { return (Settings::default(), None) };
    let mut s = match toml::from_str::<Settings>(&text) {
        Ok(s) => s,
        Err(e) => {
            // The next save would silently replace the user's keys with defaults: keep the unreadable file.
            let backup = path.with_extension("toml.bak");
            let notice = match fs::rename(&path, &backup) {
                Ok(()) => format!("Paramètres illisibles ({e}) : copie conservée dans {}", backup.display()),
                Err(_) => format!("Paramètres illisibles ({e}) : valeurs par défaut"),
            };
            return (Settings::default(), Some(notice));
        }
    };
    let keys = [&s.deepseek_key, &s.claude_key, &s.jev_key];
    let migrate = keys.iter().any(|k| !secret::is_sealed(k));
    s.deepseek_key = secret::open(&s.deepseek_key);
    s.claude_key = secret::open(&s.claude_key);
    s.jev_key = secret::open(&s.jev_key);
    if migrate && cfg!(windows) {
        let _ = save_settings(&s);
    }
    (s, None)
}

pub fn save_settings(settings: &Settings) -> Result<()> {
    write_private(&paths::settings(), toml::to_string_pretty(&sealed(settings)?)?.as_bytes())
}

/// Settings as written on disk: every API key sealed, nothing else touched.
fn sealed(settings: &Settings) -> Result<Settings> {
    Ok(Settings {
        deepseek_key: secret::seal(settings.deepseek_key.trim())?,
        claude_key: secret::seal(settings.claude_key.trim())?,
        jev_key: secret::seal(settings.jev_key.trim())?,
        ..settings.clone()
    })
}

/// Newest first.
pub fn load_conversations() -> Vec<Conversation> {
    let Ok(dir) = fs::read_dir(paths::conversations()) else { return Vec::new() };
    let mut all: Vec<Conversation> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice(&fs::read(e.path()).ok()?).ok())
        .collect();
    all.sort_by_key(|c| std::cmp::Reverse(c.updated));
    all
}

pub fn save_conversation(conv: &Conversation) -> Result<()> {
    write_private(&conversation_path(conv.id), &serde_json::to_vec(conv)?)
}

/// Removes the conversation and its file checkpoints.
pub fn delete_conversation(id: Uuid) -> Result<()> {
    let _ = fs::remove_dir_all(paths::checkpoints().join(id.to_string()));
    fs::remove_file(conversation_path(id))?;
    Ok(())
}

fn conversation_path(id: Uuid) -> std::path::PathBuf {
    paths::conversations().join(format!("{id}.json"))
}

/// Atomic write: a uniquely named temporary file (concurrent writers never share it), flushed, then renamed.
/// On Unix the file is created owner-only, so secrets are never readable by others, not even briefly.
pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", Uuid::new_v4().simple()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&tmp).and_then(|mut file| {
        file.write_all(data)?;
        file.sync_all()
    });
    let result = written.and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(result?)
}

/// Scheduled prompts; a corrupt file must never block the app.
pub fn load_jobs() -> Vec<crate::domain::schedule::Job> {
    fs::read_to_string(paths::schedules()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_jobs(jobs: &[crate::domain::schedule::Job]) -> Result<()> {
    write_private(&paths::schedules(), serde_json::to_vec_pretty(jobs)?.as_slice())
}

#[cfg(test)]
mod tests {
    use crate::domain::settings::Settings;
    #[test]
    fn no_api_key_is_ever_written_in_plaintext() {
        let s = Settings {
            deepseek_key: "sk-deepseek-secret".into(),
            claude_key: "sk-ant-secret".into(),
            jev_key: "sk-jev-secret".into(),
            ..Settings::default()
        };
        let toml = toml::to_string_pretty(&super::sealed(&s).unwrap()).unwrap();
        for needle in ["sk-deepseek-secret", "sk-ant-secret", "sk-jev-secret"] {
            assert!(!toml.contains(needle), "{needle} écrit en clair");
        }
        #[cfg(windows)]
        assert_eq!(toml.matches("dpapi:").count(), 3, "trois clés scellées");
        #[cfg(not(windows))]
        let _ = &toml;
        assert!(toml.contains("claude_auth"));
        assert!(!toml.contains("access") && !toml.contains("refresh"), "les jetons OAuth ne vont pas dans settings.toml");
    }

    #[test]
    fn an_atomic_write_replaces_the_file_and_leaves_no_temporary() {
        let dir = std::env::temp_dir().join("omega-write-private");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        super::write_private(&path, b"a = 1").unwrap();
        super::write_private(&path, b"a = 2").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a = 2");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "aucun fichier temporaire ne reste");
    }
}
