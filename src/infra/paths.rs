use std::{fs, io, path::PathBuf};

/// Everything Omega stores lives here (local only, never synced).
pub fn root() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("Omega")
}

pub fn conversations() -> PathBuf {
    root().join("conversations")
}

/// Pre-edit file copies, one folder per conversation/turn.
pub fn checkpoints() -> PathBuf {
    root().join("checkpoints")
}

/// CLI-Anything registry and SKILL.md cache.
pub fn clihub() -> PathBuf {
    root().join("clihub")
}

pub fn web_cache() -> PathBuf {
    root().join("web")
}

/// Default sandbox when the user has not picked a project folder; kept apart from the data dir (API keys).
pub fn workspace() -> PathBuf {
    dirs::document_dir().map_or_else(|| root().join("workspace"), |d| d.join("Omega"))
}

pub fn settings() -> PathBuf {
    root().join("settings.toml")
}

/// Sealed OAuth credentials, one file per subscription provider.
pub fn creds() -> PathBuf {
    root().join("credentials")
}

/// Prompts programmés (aucun secret).
pub fn schedules() -> PathBuf {
    root().join("schedules.json")
}

pub fn ensure() -> io::Result<()> {
    [conversations(), web_cache(), workspace(), creds()].iter().try_for_each(fs::create_dir_all)
}
