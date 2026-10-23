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
    infra::{paths, secret},
};

pub mod claude;
pub mod gemini;
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
    let sealed = secret::seal(&json);
    let path = dir.join(format!("{}.json", provider.id()));
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, sealed.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Missing, unreadable or undecryptable file = no credentials.
pub fn read_from(dir: &Path, provider: Provider) -> Option<Cred> {
    let raw = std::fs::read_to_string(dir.join(format!("{}.json", provider.id()))).ok()?;
    serde_json::from_str(&secret::open(raw.trim())).ok()
}

/// State of one provider's credentials
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

    /// Persists `cred` in memory and on disk.
    pub fn save(&self, cred: Cred) -> Result<()> {
        write_to(&self.dir, self.provider, &cred)?;
        *self.lock() = cred;
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
        let current = self.cred();
        if current.refresh.trim().is_empty() {
            bail!("{} : session expirée — reconnectez-vous dans ⚙ Paramètres.", self.provider.label());
        }
        let renewed = refresh(http, self.provider, &current).await?;
        let account = if renewed.account.is_empty() { current.account } else { renewed.account };
        let project = if renewed.project.is_empty() { current.project } else { renewed.project };
        self.save(Cred { account, project, ..renewed })?;
        Ok(self.access())
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
    }
}

/// `{error, error_description}` or `{error: {message}}` → one readable line.
pub fn oauth_error(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            let detail = v["error_description"].as_str().or_else(|| v["error"]["message"].as_str()).or_else(|| v["error"].as_str());
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
        Cred { access: "a".into(), refresh: "r".into(), expires_at, account: String::new(), project: String::new() }
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
        let cred = Cred { access: "at-123".into(), refresh: "rt-456".into(), expires_at: 42, account: "me@x".into(), project: "p".into() };
        super::write_to(&dir, Provider::Claude, &cred).unwrap();
        let raw = std::fs::read_to_string(dir.join("claude.json")).unwrap();
        assert!(!raw.contains("at-123") && !raw.contains("rt-456"), "jeton en clair sur disque");
        let back = super::read_from(&dir, Provider::Claude).unwrap();
        assert_eq!((back.refresh.as_str(), back.project.as_str()), ("rt-456", "p"));
        assert!(super::read_from(&dir, Provider::Gemini).is_none());
    }

    #[test]
    fn oauth_errors_are_readable() {
        assert_eq!(super::oauth_error(r#"{"error":"invalid_grant","error_description":"expired"}"#), "invalid_grant: expired");
        assert_eq!(super::oauth_error(r#"{"error":{"message":"bad"}}"#), "bad");
        assert_eq!(super::oauth_error("plain"), "plain");
    }
}
