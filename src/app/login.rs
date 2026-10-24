//! Subscription login: PKCE setup, code exchange and credential persistence.

use anyhow::{Context, Result};
use reqwest::Client;

use crate::{
    domain::auth::{Cred, Provider},
    infra::{
        auth::{claude, pkce::Pkce},
        paths,
    },
};

/// State of an in-flight OAuth dance, rendered by the UI.
#[derive(Clone)]
pub struct Login {
    pub provider: Provider,
    pub url: String,
    pub verifier: String,
    pub state: String,
    pub paste: String,
    pub status: String,
    /// An exchange is in flight; blocks a second POST that would trigger a 429.
    pub submitting: bool,
}

/// Claude: the user validates on claude.ai and pastes the code back.
pub fn claude_start() -> (String, Login) {
    let pkce = Pkce::new();
    let url = claude::authorize_url(&pkce);
    let login = Login {
        provider: Provider::Claude,
        url: url.clone(),
        verifier: pkce.verifier,
        state: pkce.state,
        paste: String::new(),
        status: String::new(),
        submitting: false,
    };
    (url, login)
}

/// Exchanges the code pasted from the provider page.
pub async fn finish_paste(http: &Client, login: &Login, code: &str) -> Result<Cred> {
    match login.provider {
        Provider::Claude => claude::exchange(http, code, &login.verifier, &login.state).await,
    }
}

/// Reads the session the official Claude Code CLI already logged in.
pub fn import_cli() -> Result<Cred> {
    let dir = claude::cli_dir().context("dossier personnel introuvable")?;
    claude::import_cli(&dir).context("session Claude Code introuvable — lancez `claude` puis `/login` avant d'importer")
}

/// Seals the credentials on disk; `Settings` only records the auth mode.
pub fn persist(provider: Provider, cred: &Cred) -> Result<()> {
    crate::infra::auth::write_to(&paths::creds(), provider, cred)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_url_is_ready_to_open() {
        let (url, login) = claude_start();
        assert!(url.starts_with("https://claude.ai/oauth/authorize?"));
        assert_eq!(login.provider, Provider::Claude);
        assert_eq!(login.state.len(), 64);
    }
}
