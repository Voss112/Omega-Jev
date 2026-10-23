use std::{fs, path::Path};

use anyhow::Result;
use uuid::Uuid;

use super::{paths, secret};
use crate::domain::{chat::Conversation, settings::Settings};

/// Keys are decrypted in memory; legacy plaintext keys are re-saved encrypted.
pub fn load_settings() -> Settings {
    let Some(mut s) = fs::read_to_string(paths::settings()).ok().and_then(|t| toml::from_str::<Settings>(&t).ok()) else {
        return Settings::default();
    };
    let migrate = [&s.kimi_key, &s.deepseek_key, &s.jev_key].iter().any(|k| !secret::is_sealed(k));
    s.kimi_key = secret::open(&s.kimi_key);
    s.deepseek_key = secret::open(&s.deepseek_key);
    s.jev_key = secret::open(&s.jev_key);
    if migrate {
        let _ = save_settings(&s);
    }
    s
}

pub fn save_settings(settings: &Settings) -> Result<()> {
    let sealed = Settings {
        kimi_key: secret::seal(settings.kimi_key.trim()),
        deepseek_key: secret::seal(settings.deepseek_key.trim()),
        jev_key: secret::seal(settings.jev_key.trim()),
        ..settings.clone()
    };
    write_private(&paths::settings(), toml::to_string_pretty(&sealed)?.as_bytes())
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
