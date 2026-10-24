//! Subscription login: PKCE setup, code exchange and credential persistence.

use anyhow::{Context, Result, bail};
use reqwest::Client;

use crate::{
    domain::auth::{Cred, Provider},
    infra::{
        auth::{claude, gemini, github, pkce::Pkce},
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
    /// Gemini: loopback redirect the browser comes back to.
    pub redirect: String,
    pub paste: String,
    pub status: String,
    /// An exchange is in flight; blocks a second POST that would trigger a 429.
    pub submitting: bool,
    /// Gemini: loopback port waiting for the browser redirect.
    pub port: u16,
    /// GitHub: code the user types on github.com/login/device.
    pub user_code: String,
}

impl Login {
    fn new(provider: Provider, url: String) -> Self {
        Self {
            provider,
            url,
            verifier: String::new(),
            state: String::new(),
            redirect: String::new(),
            paste: String::new(),
            status: String::new(),
            submitting: false,
            port: 0,
            user_code: String::new(),
        }
    }

    /// The provider page hands a code back to paste (Claude, or Gemini when the loopback is unreachable).
    pub fn accepts_paste(&self) -> bool {
        self.provider != Provider::GitHub
    }
}

/// Claude: the user validates on claude.ai and pastes the code back.
pub fn claude_start() -> (String, Login) {
    let pkce = Pkce::new();
    let url = claude::authorize_url(&pkce);
    let login = Login { verifier: pkce.verifier, state: pkce.state, ..Login::new(Provider::Claude, url.clone()) };
    (url, login)
}

/// Gemini: the browser redirect is captured on loopback, no copy-paste needed.
pub fn gemini_start() -> Result<(String, Login)> {
    let port = std::net::TcpListener::bind("127.0.0.1:0").context("port loopback indisponible")?.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}/oauth2callback");
    let pkce = Pkce::new();
    let url = gemini::authorize_url(&pkce, &redirect);
    let login = Login {
        verifier: pkce.verifier,
        state: pkce.state,
        redirect,
        port,
        status: "En attente de la validation dans le navigateur…".to_owned(),
        ..Login::new(Provider::Gemini, url.clone())
    };
    Ok((url, login))
}

/// GitHub: the device code is requested first; the modal shows it while `github_wait` polls.
pub fn github_start() -> Login {
    Login {
        status: "Demande d'un code à GitHub…".to_owned(),
        ..Login::new(Provider::GitHub, github::VERIFY.to_owned())
    }
}

/// Exchanges the code pasted from the provider page (Gemini: the code or the whole redirect URL).
pub async fn finish_paste(http: &Client, login: &Login, code: &str) -> Result<Cred> {
    match login.provider {
        Provider::Claude => claude::exchange(http, code, &login.verifier, &login.state).await,
        Provider::Gemini => gemini_connect(http, &pasted_code(code), &login.verifier, &login.redirect).await,
        Provider::GitHub => bail!("GitHub se valide sur github.com/login/device"),
    }
}

/// `…/oauth2callback?state=…&code=XYZ&scope=…` or a bare code → the code.
fn pasted_code(pasted: &str) -> String {
    let pasted = pasted.trim();
    let query = pasted.split_once('?').map_or(pasted, |(_, q)| q);
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("code="))
        .map_or_else(|| pasted.to_owned(), |c| c.replace("%2F", "/").replace("%2f", "/"))
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
    let (tier, project) = gemini::tier(http, &cred.access, "").await.context("activation Gemini Code Assist")?;
    if cred.account.is_empty() || cred.account == "google" {
        cred.account = tier;
    }
    cred.project = project;
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

/// Removes a provider's sealed credentials (GitHub "Se déconnecter").
pub fn forget(provider: Provider) -> Result<()> {
    let path = paths::creds().join(format!("{}.json", provider.id()));
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).with_context(|| format!("suppression {}", path.display())),
        _ => Ok(()),
    }
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
        assert!(login.accepts_paste());
    }

    #[test]
    fn gemini_login_listens_on_a_free_loopback_port() {
        let (url, login) = gemini_start().unwrap();
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A"));
        assert!(login.port > 0);
        assert!(url.contains(&format!("{}%2Foauth2callback", login.port)));
    }

    #[test]
    fn a_pasted_redirect_url_yields_its_code() {
        assert_eq!(pasted_code("http://127.0.0.1:5/oauth2callback?state=s&code=4%2F0Ab&scope=x"), "4/0Ab");
        assert_eq!(pasted_code(" 4/0Ab "), "4/0Ab");
        assert!(!github_start().accepts_paste());
    }
}
