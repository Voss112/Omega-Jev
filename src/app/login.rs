//! Subscription login: PKCE setup, code exchange and credential persistence.

use anyhow::{Context, Result, bail};
use reqwest::Client;

use crate::{
    domain::auth::{Cred, Provider},
    infra::{
        auth::{claude, gemini, pkce::Pkce},
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
    pub redirect: String,
    pub paste: String,
    pub status: String,
    pub listening: bool,
    /// Loopback port for the Gemini redirect.
    pub port: u16,
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
        redirect: claude::REDIRECT.to_owned(),
        paste: String::new(),
        status: String::new(),
        listening: false,
        port: 0,
    };
    (url, login)
}

/// Gemini: the browser redirect is captured on loopback, no copy-paste needed.
pub fn gemini_start() -> Result<(String, Login)> {
    let port = free_port()?;
    let redirect = format!("http://127.0.0.1:{port}/oauth2callback");
    let pkce = Pkce::new();
    let url = gemini::authorize_url(&pkce, &redirect);
    let login = Login {
        provider: Provider::Gemini,
        url: url.clone(),
        verifier: pkce.verifier,
        state: pkce.state,
        redirect,
        paste: String::new(),
        status: String::new(),
        listening: true,
        port,
    };
    Ok((url, login))
}

fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").context("port loopback indisponible")?;
    Ok(listener.local_addr()?.port())
}

/// Exchanges the pasted code (Claude) or a code typed in manually (Gemini fallback).
pub async fn finish_paste(http: &Client, login: &Login, code: &str) -> Result<Cred> {
    match login.provider {
        Provider::Claude => claude::exchange(http, code, &login.verifier, &login.state).await,
        Provider::Gemini => gemini_connect(http, code, &login.verifier, &login.redirect).await,
    }
}

/// Waits for the browser redirect, then exchanges the code.
pub async fn await_gemini(http: &Client, login: &Login) -> Result<Cred> {
    if login.port == 0 {
        bail!("port de retour manquant");
    }
    let code = crate::infra::auth::pkce::await_code(login.port, &login.state, std::time::Duration::from_secs(300)).await?;
    gemini_connect(http, &code, &login.verifier, &login.redirect).await
}

/// Exchanges the code, then attaches the Code Assist project so the first message already works.
async fn gemini_connect(http: &Client, code: &str, verifier: &str, redirect: &str) -> Result<Cred> {
    let mut cred = gemini::exchange(http, code, verifier, redirect).await?;
    if let Ok((tier, project)) = gemini::tier(http, &cred.access, "").await {
        if cred.account.is_empty() || cred.account == "google" {
            cred.account = tier;
        }
        if !project.is_empty() {
            cred.project = project;
        }
    }
    Ok(cred)
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
        assert!(!login.listening);
        assert_eq!(login.provider, Provider::Claude);
        assert_eq!(login.state.len(), 64);
    }

    #[test]
    fn gemini_login_listens_on_a_free_loopback_port() {
        let (url, login) = gemini_start().unwrap();
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A"));
        assert!(login.listening && login.port > 0);
        assert!(url.contains(&format!("{}%2Foauth2callback", login.port)));
    }
}
