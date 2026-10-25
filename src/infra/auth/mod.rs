//! OAuth credentials for consumer subscriptions: sealed at rest, refreshed transparently.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
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
/// First pause after a refused refresh; it doubles with each consecutive refusal up to `PAUSE_MAX`.
const PAUSE_FIRST: Duration = Duration::from_secs(60);
const PAUSE_MAX: Duration = Duration::from_secs(15 * 60);

/// The token endpoint answered 429. `wait`: the server's `Retry-After`, then the pause Omega actually keeps.
#[derive(Debug)]
pub struct RateLimited {
    pub provider: Provider,
    pub wait: Option<Duration>,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let secs = self.wait.map_or(60, |w| w.as_secs().max(1));
        let when = if secs >= 120 { format!("{} min", secs.div_ceil(60)) } else { format!("{secs} s") };
        let (en, label) = (crate::ui::english(), self.provider.label());
        let instead = match (self.provider, en) {
            (Provider::Claude, false) => {
                " Sans attendre : importez la session Claude Code ou collez un jeton d'abonnement (claude setup-token → sk-ant-oat…)."
            }
            (Provider::Claude, true) => {
                " To skip the wait: import the Claude Code session or paste a subscription token (claude setup-token → sk-ant-oat…)."
            }
            _ => "",
        };
        if en {
            write!(f, "{label} is rate-limiting token renewals from this connection {NOT_QUOTA_EN}. Omega retries in {when}.{instead}")
        } else {
            write!(f, "{label} limite le renouvellement des jetons depuis cette connexion {NOT_QUOTA_FR}. Omega réessaie dans {when}.{instead}")
        }
    }
}

impl std::error::Error for RateLimited {}

const NOT_QUOTA_FR: &str = "(429) — ce n'est pas votre quota";
const NOT_QUOTA_EN: &str = "(429) — not your plan quota";

/// A turn failed on a token-renewal 429 (`RateLimited`), not on the plan limit: the quota is not reached.
pub fn renewal_throttled(error: &str) -> bool {
    error.contains(NOT_QUOTA_FR) || error.contains(NOT_QUOTA_EN)
}

/// `Retry-After` in seconds (the form OAuth servers send); anything else means "Omega's own schedule".
fn retry_after(res: &reqwest::Response) -> Option<Duration> {
    res.headers().get("retry-after")?.to_str().ok()?.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Last refused refresh of a provider, and how long every handle leaves the token endpoint alone: polls,
/// warm-ups and requests retrying at once would keep a rate limit alive forever.
struct Pause {
    until: Option<Instant>,
    strikes: u32,
    error: String,
    throttled: bool,
}

impl Pause {
    const fn new() -> Self {
        Self { until: None, strikes: 0, error: String::new(), throttled: false }
    }

    /// The standing refusal with its remaining time; `None` once the pause is over.
    fn active(&self, provider: Provider, now: Instant) -> Option<anyhow::Error> {
        let left = self.until.map(|until| until.saturating_duration_since(now)).filter(|left| !left.is_zero())?;
        Some(if self.throttled { RateLimited { provider, wait: Some(left) }.into() } else { anyhow::anyhow!("{}", self.error) })
    }

    /// A pending 429 only: a login still goes through after any other refusal (it is the cure for it).
    fn throttle(&self, provider: Provider, now: Instant) -> Option<anyhow::Error> {
        self.active(provider, now).filter(|_| self.throttled)
    }

    /// Records a refused refresh and returns the error to report (a 429 carries the pause actually kept).
    /// A network failure (offline, timeout) pauses nothing: the next attempt may well succeed.
    fn fail(&mut self, provider: Provider, error: anyhow::Error, now: Instant) -> anyhow::Error {
        if error.chain().any(|e| e.is::<reqwest::Error>()) {
            return error;
        }
        let server = error.downcast_ref::<RateLimited>().map(|r| r.wait);
        let doubled = PAUSE_FIRST.saturating_mul(1 << self.strikes.min(8)).min(PAUSE_MAX);
        let wait = server.flatten().map_or(doubled, |w| w.clamp(Duration::from_secs(1), Duration::from_secs(3600)));
        self.until = Some(now + wait);
        self.strikes = self.strikes.saturating_add(1);
        self.throttled = server.is_some();
        self.error = format!("{error:#}");
        if self.throttled { RateLimited { provider, wait: Some(wait) }.into() } else { error }
    }

    fn clear(&mut self) {
        *self = Self::new();
    }
}

/// Process-wide, per provider: every `Auth` handle (chat, quota poll, warm-up, model list) shares it.
fn paused(provider: Provider) -> MutexGuard<'static, Pause> {
    static PAUSES: [Mutex<Pause>; Provider::ALL.len()] = [const { Mutex::new(Pause::new()) }; Provider::ALL.len()];
    PAUSES[provider as usize].lock().unwrap_or_else(PoisonError::into_inner)
}

/// Credentials another program already renewed on this machine (Claude Code's shared session).
fn adopted(provider: Provider, cred: &Cred) -> Option<Cred> {
    match provider {
        Provider::Claude => claude::adopt(cred),
        Provider::Gemini | Provider::GitHub => None,
    }
}

/// A still-valid session to fall back on when a refresh is refused (Claude Code's own one).
fn fallback(provider: Provider) -> Option<Cred> {
    match provider {
        Provider::Claude => claude::cli_session(),
        Provider::Gemini | Provider::GitHub => None,
    }
}

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
        // Renewed by another program (Claude Code's shared session): no request at all.
        let renewed = match adopted(self.provider, &current) {
            Some(cred) => cred,
            None => self.renew(http, &current).await?,
        };
        let account = if renewed.account.is_empty() { current.account } else { renewed.account };
        let project = if renewed.project.is_empty() { current.project } else { renewed.project };
        let cred = Cred { account, project, ..renewed };
        // Memory first: the previous refresh token is spent, it must never be used again even if the disk write fails.
        self.replace(cred.clone());
        write_to(&self.dir, self.provider, &cred)?;
        Ok(cred.access)
    }

    /// Network refresh, unless the provider refused one a moment ago: asking again would only extend a rate
    /// limit or repeat the same refusal. Meanwhile a valid Claude Code session stands in.
    async fn renew(&self, http: &Client, current: &Cred) -> Result<Cred> {
        let standing = paused(self.provider).active(self.provider, Instant::now());
        if let Some(error) = standing {
            return fallback(self.provider).ok_or(error);
        }
        if current.refresh.trim().is_empty() && !current.cli {
            bail!("{} : session expirée — reconnectez-vous dans ⚙ Paramètres.", self.provider.label());
        }
        match refresh(http, self.provider, current).await {
            Ok(renewed) => {
                paused(self.provider).clear();
                Ok(renewed)
            }
            Err(e) => {
                let error = paused(self.provider).fail(self.provider, e, Instant::now());
                fallback(self.provider).ok_or(error)
            }
        }
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
    fn a_refused_refresh_pauses_every_handle_and_doubles() {
        use std::time::{Duration, Instant};
        let (now, mut pause) = (Instant::now(), super::Pause::new());
        assert!(pause.active(Provider::Claude, now).is_none());
        let refused = pause.fail(Provider::Claude, anyhow::anyhow!("Claude 400 : invalid_grant"), now);
        assert!(refused.to_string().contains("invalid_grant"));
        assert!(pause.active(Provider::Claude, now + Duration::from_secs(59)).is_some_and(|e| e.to_string().contains("invalid_grant")));
        assert!(pause.active(Provider::Claude, now + Duration::from_secs(61)).is_none(), "1 min");
        assert!(pause.throttle(Provider::Claude, now).is_none(), "un refus n'empêche pas de se reconnecter");
        // Second refusal: twice as long; a 429 then carries the pause actually kept.
        let limited = super::RateLimited { provider: Provider::Claude, wait: None };
        let reported = pause.fail(Provider::Claude, limited.into(), now);
        assert!(reported.to_string().contains("2 min") && super::renewal_throttled(&reported.to_string()), "{reported}");
        assert!(pause.throttle(Provider::Claude, now + Duration::from_secs(119)).is_some());
        assert!(pause.active(Provider::Claude, now + Duration::from_secs(121)).is_none());
        // The server's own delay wins; a success clears everything.
        let told = super::RateLimited { provider: Provider::Claude, wait: Some(Duration::from_secs(7)) };
        assert!(pause.fail(Provider::Claude, told.into(), now).to_string().contains("7 s"));
        pause.clear();
        assert!(pause.active(Provider::Claude, now).is_none());
        for _ in 0..20 {
            pause.fail(Provider::Claude, anyhow::anyhow!("x"), now);
        }
        assert!(pause.active(Provider::Claude, now + super::PAUSE_MAX + Duration::from_secs(1)).is_none(), "plafond");
    }

    #[tokio::test]
    async fn a_network_failure_pauses_nothing() {
        let http = crate::infra::http::client("").unwrap();
        // Port 9 (discard) on a public test address: the connection itself fails, no server answers.
        let offline = http.get("https://192.0.2.1:9/").timeout(std::time::Duration::from_millis(50)).send().await.unwrap_err();
        let mut pause = super::Pause::new();
        pause.fail(Provider::Gemini, anyhow::Error::new(offline).context("connexion"), std::time::Instant::now());
        assert!(pause.active(Provider::Gemini, std::time::Instant::now()).is_none());
    }

    #[test]
    fn oauth_errors_are_readable() {
        assert_eq!(super::oauth_error(r#"{"error":"invalid_grant","error_description":"expired"}"#), "invalid_grant: expired");
        assert_eq!(super::oauth_error(r#"{"error":{"message":"bad"}}"#), "bad");
        assert_eq!(super::oauth_error("plain"), "plain");
    }
}
