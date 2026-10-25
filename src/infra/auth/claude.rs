//! Claude consumer subscription: OAuth PKCE against the public Claude Code client.

use std::{path::Path, time::Instant};

use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{Value, json};

use super::{RateLimited, oauth_error, pkce};
use crate::domain::auth::{Cred, Provider, Window};

// Same endpoints as the official Claude Code CLI 2.1 (console.anthropic.com moved to platform.claude.com).
pub const AUTHORIZE: &str = "https://claude.com/cai/oauth/authorize";
pub const TOKEN: &str = "https://platform.claude.com/v1/oauth/token";
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const REDIRECT: &str = "https://platform.claude.com/oauth/code/callback";
/// Only what Omega uses: inference and the plan usage (no API-key creation right).
pub const SCOPE: &str = "user:profile user:inference";
/// Required on subscription tokens; the API rejects them otherwise.
pub const BETA: &str = "oauth-2025-04-20";
/// Subscription tokens are only honoured when the first system block is exactly this.
pub const SYSTEM_PREFIX: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
pub const API: &str = "https://api.anthropic.com";
pub const USAGE: &str = "https://api.anthropic.com/api/oauth/usage";
pub const VERSION: &str = "2023-06-01";
/// Session file of the official CLI, inside `cli_dir()`.
const CLI_FILE: &str = ".credentials.json";

/// Browser URL the user validates on claude.ai, then pastes the displayed code back.
pub fn authorize_url(p: &pkce::Pkce) -> String {
    format!(
        "{AUTHORIZE}?code=true&client_id={CLIENT_ID}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        pkce::encode(REDIRECT),
        pkce::encode(SCOPE),
        p.challenge,
        p.state
    )
}

/// The page shows `code#state`; a bare code is accepted too.
pub fn split_code(pasted: &str) -> (String, String) {
    match pasted.trim().split_once('#') {
        Some((code, state)) => (code.trim().to_owned(), state.trim().to_owned()),
        None => (pasted.trim().to_owned(), String::new()),
    }
}

pub async fn exchange(http: &Client, pasted: &str, verifier: &str, default_state: &str) -> Result<Cred> {
    // A pending 429 would only be extended by another request: Claude Code's session stands in meanwhile.
    let pending = super::paused(Provider::Claude).throttle(Provider::Claude, Instant::now());
    if let Some(error) = pending {
        return cli_session().ok_or(error);
    }
    let (code, state) = split_code(pasted);
    let state = if state.is_empty() { default_state.to_owned() } else { state };
    let body = json!({
        "grant_type": "authorization_code",
        "code": code,
        "state": state,
        "client_id": CLIENT_ID,
        "redirect_uri": REDIRECT,
        "code_verifier": verifier,
    });
    match post(http, &body).await {
        Ok(v) => {
            let cred = cred_of(&v)?;
            let account = if cred.account.is_empty() { "claude".to_owned() } else { cred.account };
            Ok(Cred { account, ..cred })
        }
        Err(e) if e.is::<RateLimited>() => {
            let error = super::paused(Provider::Claude).fail(Provider::Claude, e, Instant::now());
            cli_session().ok_or(error)
        }
        Err(e) => Err(e),
    }
}

/// Renews through the token endpoint. A session shared with Claude Code uses the CLI's latest refresh token (it
/// rotates on every refresh) and writes the new pair back, as Claude Code does between its own processes.
pub async fn refresh(http: &Client, cred: &Cred) -> Result<Cred> {
    let dir = cli_dir();
    let cli = dir.as_deref().and_then(import_cli).filter(|c| shared(cred, c));
    let spent = cli.as_ref().map_or(cred.refresh.as_str(), |c| c.refresh.as_str()).to_owned();
    anyhow::ensure!(!spent.trim().is_empty(), "Claude : session expirée — reconnectez-vous dans ⚙ Paramètres.");
    let body = json!({ "grant_type": "refresh_token", "refresh_token": spent, "client_id": CLIENT_ID });
    let v = post(http, &body).await?;
    let mut renewed = cred_of(&v)?;
    if renewed.refresh.is_empty() {
        renewed.refresh.clone_from(&spent);
    }
    renewed.cli = cli.is_some();
    if let (Some(_), Some(dir)) = (&cli, &dir) {
        write_back(&dir.join(CLI_FILE), &spent, &renewed, &v);
    }
    Ok(renewed)
}

/// One token-endpoint call: a 429 becomes `RateLimited` (the caller pauses), any other refusal a readable error.
async fn post(http: &Client, body: &Value) -> Result<Value> {
    let res = http
        .post(TOKEN)
        .header("content-type", "application/json")
        .json(body)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .context("connexion au serveur de jetons Claude")?;
    let status = res.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(RateLimited { provider: Provider::Claude, wait: super::retry_after(&res) }.into());
    }
    let text = res.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("Claude {status} : {}", oauth_error(&text));
    }
    serde_json::from_str(&text).context("réponse de jeton illisible")
}

/// Credentials out of a token answer; `account` stays empty when the answer names none (the caller keeps its own).
fn cred_of(v: &Value) -> Result<Cred> {
    let access = v["access_token"].as_str().unwrap_or_default().to_owned();
    anyhow::ensure!(!access.is_empty(), "réponse de jeton sans access_token");
    let expires_in = v["expires_in"].as_i64().filter(|s| *s > 0).unwrap_or(3600);
    Ok(Cred {
        access,
        refresh: v["refresh_token"].as_str().unwrap_or_default().to_owned(),
        expires_at: chrono::Utc::now().timestamp().saturating_add(expires_in),
        account: v["account"]["email_address"].as_str().or_else(|| v["subscriptionType"].as_str()).unwrap_or_default().to_owned(),
        ..Cred::default()
    })
}

/// Imported from Claude Code, or still holding the refresh token Claude Code has on disk.
fn shared(cred: &Cred, cli: &Cred) -> bool {
    cred.cli || (!cred.refresh.is_empty() && cred.refresh == cli.refresh)
}

/// Claude Code renewed the shared session already: its token is taken as is, without any request.
pub fn adopt(cred: &Cred) -> Option<Cred> {
    cli_session().filter(|c| shared(cred, c) && c.access != cred.access)
}

/// The session Claude Code keeps on this machine, while it stays valid for a while.
pub fn cli_session() -> Option<Cred> {
    import_cli(&cli_dir()?).filter(|c| usable(c, chrono::Utc::now().timestamp()))
}

fn usable(cred: &Cred, now: i64) -> bool {
    cred.expires_at == 0 || cred.expires_at - now > 60
}

/// Puts the renewed pair back into Claude Code's file, only if it still holds the token just spent (another
/// process may have renewed it meanwhile): the compare-and-swap Claude Code applies between its own processes.
/// Without it, Claude Code's next refresh would spend a dead token and sign it out.
fn write_back(path: &Path, spent: &str, renewed: &Cred, answer: &Value) {
    let Some(mut v) = std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str::<Value>(&raw).ok()) else {
        return;
    };
    let Some(oauth) = v.get_mut("claudeAiOauth").and_then(Value::as_object_mut) else { return };
    if oauth.get("refreshToken").and_then(Value::as_str) != Some(spent) {
        return;
    }
    oauth.insert("accessToken".into(), json!(renewed.access));
    oauth.insert("refreshToken".into(), json!(renewed.refresh));
    oauth.insert("expiresAt".into(), json!(renewed.expires_at.saturating_mul(1000)));
    // Claude Code drops the refresh-token expiry when the server sends none for the new token.
    match answer["refresh_token_expires_in"].as_i64() {
        Some(secs) => oauth.insert("refreshTokenExpiresAt".into(), json!(chrono::Utc::now().timestamp().saturating_add(secs) * 1000)),
        None => oauth.remove("refreshTokenExpiresAt"),
    };
    if let Ok(bytes) = serde_json::to_vec(&v) {
        let _ = crate::infra::store::write_private(path, &bytes);
    }
}

/// A long-lived subscription token (`claude setup-token` → `sk-ant-oat01-…`): no exchange, no IP rate limit.
pub fn import_token(token: &str) -> Result<Cred> {
    let token = token.trim();
    anyhow::ensure!(!token.is_empty(), "jeton vide");
    anyhow::ensure!(token.starts_with("sk-ant-oat"), "jeton attendu : sk-ant-oat… (claude setup-token)");
    Ok(Cred {
        access: token.to_owned(),
        expires_at: chrono::Utc::now().timestamp() + 300 * 24 * 3600,
        account: "jeton d'abonnement".to_owned(),
        ..Cred::default()
    })
}

/// Imports the session the official Claude Code CLI already logged in (`<dir>/.credentials.json`).
pub fn import_cli(dir: &Path) -> Option<Cred> {
    let raw = std::fs::read_to_string(dir.join(CLI_FILE)).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let oauth = &v["claudeAiOauth"];
    let access = oauth["accessToken"].as_str().filter(|a| !a.is_empty())?.to_owned();
    Some(Cred {
        access,
        refresh: oauth["refreshToken"].as_str().unwrap_or_default().to_owned(),
        expires_at: oauth["expiresAt"].as_i64().map_or(0, |ms| ms / 1000),
        account: oauth["subscriptionType"].as_str().unwrap_or("claude-code").to_owned(),
        project: String::new(),
        cli: true,
    })
}

/// Best-effort plan usage; any failure hides the section instead of failing the app.
pub async fn usage(http: &Client, access: &str) -> Option<Vec<Window>> {
    let res = http
        .get(USAGE)
        .header("authorization", format!("Bearer {access}"))
        .header("anthropic-beta", BETA)
        .header("anthropic-version", VERSION)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    if !res.status().is_success() {
        return None;
    }
    let v: Value = res.json().await.ok()?;
    Some(windows(&v)).filter(|w| !w.is_empty())
}

/// Lenient parser: only the documented window keys become rows.
pub fn windows(v: &Value) -> Vec<Window> {
    [
        ("five_hour", "5 h"),
        ("seven_day", crate::ui::tr("7 jours", "7 days")),
        ("seven_day_opus", crate::ui::tr("7 j Opus", "7 d Opus")),
        ("seven_day_sonnet", crate::ui::tr("7 j Sonnet", "7 d Sonnet")),
    ]
    .into_iter()
    .filter_map(|(key, label)| {
        let w = &v[key];
        let utilization = w["utilization"].as_f64()?;
        Some(Window {
            label: label.to_owned(),
            fraction: (utilization / 100.0).clamp(0.0, 1.0) as f32,
            resets: w["resets_at"].as_str().map(str::to_owned),
        })
    })
    .collect()
}

/// Default on-disk location of the official CLI credentials.
pub fn cli_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_carries_pkce_and_scope() {
        let p = pkce::Pkce::new();
        let url = authorize_url(&p);
        assert!(url.starts_with("https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e"));
        for needle in [
            "response_type=code",
            "code_challenge_method=S256",
            "redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback",
        ] {
            assert!(url.contains(needle), "manque {needle}");
        }
        assert!(url.contains(&format!("code_challenge={}", p.challenge)));
        assert!(url.contains(&format!("state={}", p.state)));
        assert!(url.contains("scope=user%3Aprofile%20user%3Ainference"));
        assert!(!url.contains("create_api_key"), "aucun droit de création de clé API");
    }

    #[test]
    fn pasted_code_may_carry_its_state() {
        assert_eq!(split_code("abc#def"), ("abc".into(), "def".into()));
        assert_eq!(split_code(" abc \n"), ("abc".into(), String::new()));
    }

    #[test]
    fn a_long_lived_token_is_accepted_without_exchange() {
        let cred = import_token("  sk-ant-oat01-abc  ").unwrap();
        assert_eq!((cred.access.as_str(), cred.refresh.as_str()), ("sk-ant-oat01-abc", ""));
        assert!(cred.expires_at > chrono::Utc::now().timestamp() + 200 * 24 * 3600);
        assert!(import_token("sk-ant-api03-x").is_err(), "clé API : mauvais mode");
        assert!(import_token("  ").is_err());
    }

    #[test]
    fn cli_credentials_are_imported_in_seconds() {
        let dir = std::env::temp_dir().join("omega-claude-cli");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"at","refreshToken":"rt","expiresAt":1700000000000,"subscriptionType":"max"}}"#,
        )
        .unwrap();
        let c = import_cli(&dir).unwrap();
        assert_eq!((c.access.as_str(), c.refresh.as_str(), c.expires_at, c.account.as_str()), ("at", "rt", 1_700_000_000, "max"));
        assert!(c.cli, "session partagée avec Claude Code");
        assert!(import_cli(&dir.join("nope")).is_none());
    }

    #[test]
    fn only_a_fresh_cli_session_replaces_a_rate_limited_exchange() {
        let now = 1_000;
        let fresh = Cred { expires_at: now + 3_600, ..Cred::default() };
        let stale = Cred { expires_at: now + 30, ..Cred::default() };
        assert!(usable(&fresh, now));
        assert!(!usable(&stale, now));
        assert!(usable(&Cred::default(), now), "expiration inconnue acceptée");
    }

    #[test]
    fn a_session_is_shared_when_imported_or_holding_the_cli_refresh_token() {
        let cli = Cred { refresh: "rt-cli".into(), cli: true, ..Cred::default() };
        assert!(shared(&Cred { cli: true, refresh: "old".into(), ..Cred::default() }, &cli), "importée puis tournée par le CLI");
        assert!(shared(&Cred { refresh: "rt-cli".into(), ..Cred::default() }, &cli), "import antérieur au marqueur");
        assert!(!shared(&Cred { refresh: "rt-omega".into(), ..Cred::default() }, &cli), "connexion propre à Omega");
        assert!(!shared(&Cred::default(), &Cred::default()), "deux jetons vides ne prouvent rien");
    }

    #[test]
    fn the_renewed_pair_goes_back_only_over_the_token_just_spent() {
        let dir = std::env::temp_dir().join(format!("omega-claude-writeback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CLI_FILE);
        let original = r#"{"claudeAiOauth":{"accessToken":"a1","refreshToken":"r1","expiresAt":1,"refreshTokenExpiresAt":2,"scopes":["user:inference"]},"organizationUuid":"org"}"#;
        std::fs::write(&path, original).unwrap();
        let renewed = Cred { access: "a2".into(), refresh: "r2".into(), expires_at: 1_800_000_000, ..Cred::default() };
        write_back(&path, "r-other", &renewed, &json!({}));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original, "renouvelée ailleurs entre-temps : fichier intact");
        write_back(&path, "r1", &renewed, &json!({}));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let oauth = &v["claudeAiOauth"];
        assert_eq!((oauth["accessToken"].as_str(), oauth["refreshToken"].as_str()), (Some("a2"), Some("r2")));
        assert_eq!(oauth["expiresAt"].as_i64(), Some(1_800_000_000_000));
        assert!(oauth.get("refreshTokenExpiresAt").is_none(), "expiration de l'ancien jeton retirée");
        assert_eq!((oauth["scopes"][0].as_str(), v["organizationUuid"].as_str()), (Some("user:inference"), Some("org")), "le reste est conservé");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_token_answer_names_its_account_or_leaves_it_to_the_caller() {
        let v = json!({ "access_token": "at", "refresh_token": "rt", "expires_in": 28_800, "account": { "email_address": "me@x" } });
        let c = cred_of(&v).unwrap();
        assert_eq!((c.access.as_str(), c.refresh.as_str(), c.account.as_str()), ("at", "rt", "me@x"));
        assert!(c.expires_at > chrono::Utc::now().timestamp() + 28_000);
        assert_eq!(cred_of(&json!({ "access_token": "at" })).unwrap().account, "", "le libellé existant est gardé");
        assert!(cred_of(&json!({})).is_err());
    }

    #[test]
    fn usage_windows_are_read_leniently() {
        let v = json!({ "five_hour": { "utilization": 42.5, "resets_at": "2026-10-24T02:00:00Z" }, "weird": 3 });
        let w = windows(&v);
        assert_eq!(w.len(), 1);
        assert!((w[0].fraction - 0.425).abs() < 1e-6 && w[0].resets.as_deref() == Some("2026-10-24T02:00:00Z"));
        assert!(windows(&json!({})).is_empty());
    }
}
