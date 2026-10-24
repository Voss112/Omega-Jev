//! OAuth credentials for consumer subscriptions: sealed at rest, refreshed transparently.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::Value;

use crate::{
    domain::auth::{Cred, Provider},
    infra::{paths, secret, store},
};

pub mod claude;
pub mod gemini;
pub mod github;
pub mod pkce;

/// A token is refreshed this many seconds before it expires, so a request never races expiry.
const MARGIN: i64 = 120;

/// base64url without padding (PKCE verifier, challenge, hashes).
pub fn b64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// True when the access token must be refreshed before the next request.
pub fn stale(cred: &Cred, now: i64) -> bool {
    cred.expires_at == 0 || cred.expires_at - now < MARGIN
}

/// Sealed on-disk representation of `cred`, one JSON line inside DPAPI.
pub fn write_to(dir: &Path, provider: Provider, cred: &Cred) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("création {}", dir.display()))?;
    let json = serde_json::to_string(cred)?;
    let sealed = secret::seal(&json)?;
    store::write_private(&dir.join(format!("{}.json", provider.id())), sealed.as_bytes())
}

/// Missing, unreadable or undecryptable file = no credentials.
pub fn read_from(dir: &Path, provider: Provider) -> Option<Cred> {
    let raw = std::fs::read_to_string(dir.join(format!("{}.json", provider.id()))).ok()?;
    serde_json::from_str(&secret::open(raw.trim())).ok()
}

/// Serialises refreshes per provider across every handle (chat, health checks, usage, model list):
/// providers rotate refresh tokens, so two parallel refreshes with the same token burn the session.
fn gate(provider: Provider) -> &'static tokio::sync::Mutex<()> {
    static GATES: [tokio::sync::Mutex<()>; Provider::ALL.len()] =
        [const { tokio::sync::Mutex::const_new(()) }; Provider::ALL.len()];
    &GATES[provider as usize]
}

/// Handle on one provider's credentials: reads, refreshes and persists them.
pub struct Auth {
    provider: Provider,
    dir: PathBuf,
    state: Mutex<Cred>,
}

impl Auth {
    /// `None` when the provider has no stored credentials.
    pub fn load(provider: Provider) -> Option<Arc<Self>> {
        let dir = paths::creds();
        let cred = read_from(&dir, provider)?;
        Some(Arc::new(Self { provider, dir, state: Mutex::new(cred) }))
    }

    /// In-memory only (fresh login before the file is written).
    pub fn access(&self) -> String {
        self.lock().access.clone()
    }

    pub fn account(&self) -> String {
        self.lock().account.clone()
    }

    pub fn project(&self) -> String {
        self.lock().project.clone()
    }

    pub fn cred(&self) -> Cred {
        self.lock().clone()
    }

    pub fn replace(&self, cred: Cred) {
        *self.lock() = cred;
    }

    /// Persists `cred` on disk, then in memory.
    pub fn save(&self, cred: Cred) -> Result<()> {
        write_to(&self.dir, self.provider, &cred)?;
        self.replace(cred);
        Ok(())
    }

    pub fn needs_refresh(&self, now: i64) -> bool {
        stale(&self.lock(), now)
    }

    /// A valid access token, refreshed and persisted first when needed.
    pub async fn fresh(&self, http: &Client) -> Result<String> {
        let now = chrono::Utc::now().timestamp();
        if !self.needs_refresh(now) {
            return Ok(self.access());
        }
        // One refresher at a time; the others pick up the renewed token from disk when they get the gate.
        let _gate = gate(self.provider).lock().await;
        let now = chrono::Utc::now().timestamp();
        let current = self.cred();
        if let Some(disk) = read_from(&self.dir, self.provider)
            && disk.access != current.access
            && !stale(&disk, now)
        {
            self.replace(disk);
            return Ok(self.access());
        }
        if !self.needs_refresh(now) {
            return Ok(self.access());
        }
        if current.refresh.trim().is_empty() {
            bail!("{} : session expirée — reconnectez-vous dans ⚙ Paramètres.", self.provider.label());
        }
        let renewed = refresh(http, self.provider, &current).await?;
        let account = if renewed.account.is_empty() { current.account } else { renewed.account };
        let project = if renewed.project.is_empty() { current.project } else { renewed.project };
        let cred = Cred { account, project, ..renewed };
        // Memory first: the previous refresh token is spent, it must never be used again even if the disk write fails.
        self.replace(cred.clone());
        write_to(&self.dir, self.provider, &cred)?;
        Ok(cred.access)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Cred> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Refreshes expired credentials through the provider's token endpoint.
pub async fn refresh(http: &Client, provider: Provider, cred: &Cred) -> Result<Cred> {
    match provider {
        Provider::Claude => claude::refresh(http, cred).await,
        Provider::Gemini => gemini::refresh(http, cred).await,
        Provider::GitHub => bail!("GitHub : jeton refusé — reconnectez-vous dans ⚙ Paramètres."),
    }
}

/// `{error, error_description}` or `{error: {message}}` → one readable line.
pub fn oauth_error(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            let detail =
                v["error_description"].as_str().or_else(|| v["error"]["message"].as_str()).or_else(|| v["error"].as_str());
            let code = v["error"].as_str();
            Some(match (code, detail) {
                (Some(c), Some(d)) if d != c => format!("{c}: {d}"),
                (_, Some(d)) => d.to_owned(),
                (Some(c), None) => c.to_owned(),
                (None, None) => return None,
            })
        })
        .unwrap_or_else(|| body.chars().take(300).collect())
}

#[cfg(test)]
mod tests {
    use crate::domain::auth::{Cred, Provider};

    fn cred(expires_at: i64) -> Cred {
        Cred { access: "a".into(), refresh: "r".into(), expires_at, ..Cred::default() }
    }

    #[test]
    fn stale_tokens_are_refreshed_before_expiry() {
        assert!(super::stale(&cred(1_000), 900));
        assert!(!super::stale(&cred(1_000), 800));
        assert!(super::stale(&cred(0), 1), "unknown expiry means refresh");
    }

    #[test]
    fn cred_round_trips_through_the_sealed_file() {
        let dir = std::env::temp_dir().join("omega-creds-roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let cred = Cred { access: "at-123".into(), refresh: "rt-456".into(), expires_at: 42, account: "me@x".into(), ..Cred::default() };
        super::write_to(&dir, Provider::Claude, &cred).unwrap();
        let raw = std::fs::read_to_string(dir.join("claude.json")).unwrap();
        assert!(!raw.contains("at-123") && !raw.contains("rt-456"), "jeton en clair sur disque");
        let back = super::read_from(&dir, Provider::Claude).unwrap();
        assert_eq!((back.refresh.as_str(), back.account.as_str()), ("rt-456", "me@x"));
    }

    #[tokio::test]
    async fn a_handle_adopts_tokens_another_handle_already_renewed() {
        let dir = std::env::temp_dir().join("omega-creds-shared");
        let _ = std::fs::remove_dir_all(&dir);
        let now = chrono::Utc::now().timestamp();
        let spent = Cred { access: "a1".into(), refresh: "r1".into(), expires_at: now - 10, ..Cred::default() };
        let renewed = Cred { access: "a2".into(), refresh: "r2".into(), expires_at: now + 3_600, ..Cred::default() };
        super::write_to(&dir, Provider::Claude, &renewed).unwrap();
        let stale = super::Auth { provider: Provider::Claude, dir, state: std::sync::Mutex::new(spent) };
        // No network: the renewed pair on disk wins instead of a second refresh with the spent token.
        let token = stale.fresh(&crate::infra::http::client("").unwrap()).await.unwrap();
        assert_eq!(token, "a2");
        assert_eq!(stale.cred().refresh, "r2");
    }

    #[test]
    fn oauth_errors_are_readable() {
        assert_eq!(super::oauth_error(r#"{"error":"invalid_grant","error_description":"expired"}"#), "invalid_grant: expired");
        assert_eq!(super::oauth_error(r#"{"error":{"message":"bad"}}"#), "bad");
        assert_eq!(super::oauth_error("plain"), "plain");
    }
}
