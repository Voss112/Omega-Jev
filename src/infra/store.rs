use std::{fs, path::Path};

use anyhow::Result;
use uuid::Uuid;

use super::{paths, secret};
use crate::domain::{chat::Conversation, settings::Settings};

/// API keys are decrypted in memory; legacy plaintext keys are re-saved encrypted.
/// Subscription tokens never live here: they are sealed in `credentials/*.json`.
pub fn load_settings() -> Settings {
    let Some(mut s) = fs::read_to_string(paths::settings()).ok().and_then(|t| toml::from_str::<Settings>(&t).ok()) else {
        return Settings::default();
    };
    let keys = [&s.kimi_key, &s.deepseek_key, &s.claude_key, &s.gemini_key, &s.jev_key];
    let migrate = keys.iter().any(|k| !secret::is_sealed(k));
    s.kimi_key = secret::open(&s.kimi_key);
    s.deepseek_key = secret::open(&s.deepseek_key);
    s.claude_key = secret::open(&s.claude_key);
    s.gemini_key = secret::open(&s.gemini_key);
    s.jev_key = secret::open(&s.jev_key);
    if migrate {
        let _ = save_settings(&s);
    }
    s
}

pub fn save_settings(settings: &Settings) -> Result<()> {
    write_private(&paths::settings(), toml::to_string_pretty(&sealed(settings))?.as_bytes())
}

/// Settings as written on disk: every API key sealed, nothing else touched.
fn sealed(settings: &Settings) -> Settings {
    Settings {
        kimi_key: secret::seal(settings.kimi_key.trim()),
        deepseek_key: secret::seal(settings.deepseek_key.trim()),
        claude_key: secret::seal(settings.claude_key.trim()),
        gemini_key: secret::seal(settings.gemini_key.trim()),
        jev_key: secret::seal(settings.jev_key.trim()),
        ..settings.clone()
    }
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

/// Atomic write (tmp + rename), owner-only permissions on Unix.
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
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
            kimi_key: "sk-kimi-secret".into(),
            deepseek_key: "sk-deepseek-secret".into(),
            claude_key: "sk-ant-secret".into(),
            gemini_key: "AIza-secret".into(),
            jev_key: "sk-jev-secret".into(),
            ..Settings::default()
        };
        let toml = toml::to_string_pretty(&super::sealed(&s)).unwrap();
        for needle in ["sk-kimi-secret", "sk-deepseek-secret", "sk-ant-secret", "AIza-secret", "sk-jev-secret"] {
            assert!(!toml.contains(needle), "{needle} écrit en clair");
        }
        #[cfg(windows)]
        assert_eq!(toml.matches("dpapi:").count(), 5, "cinq clés scellées");
        #[cfg(not(windows))]
        let _ = &toml;
        assert!(toml.contains("claude_auth"));
        assert!(!toml.contains("access") && !toml.contains("refresh"), "les jetons OAuth ne vont pas dans settings.toml");
    }
}
