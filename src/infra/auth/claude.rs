//! Claude consumer subscription: OAuth PKCE against the public Claude Code client.

use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{Value, json};

use super::{oauth_error, pkce};
use crate::domain::auth::{Cred, Window};

pub const AUTHORIZE: &str = "https://claude.ai/oauth/authorize";
pub const TOKEN: &str = "https://console.anthropic.com/v1/oauth/token";
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const REDIRECT: &str = "https://console.anthropic.com/oauth/code/callback";
pub const SCOPE: &str = "org:create_api_key user:profile user:inference";
/// Required on subscription tokens; the API rejects them otherwise.
pub const BETA: &str = "oauth-2025-04-20";
/// Subscription tokens are only honoured when the first system block is exactly this.
pub const SYSTEM_PREFIX: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
pub const API: &str = "https://api.anthropic.com";
pub const USAGE: &str = "https://api.anthropic.com/api/oauth/usage";
pub const VERSION: &str = "2023-06-01";

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

pub async fn exchange(http: &Client, pasted: &str, p: &pkce::Pkce) -> Result<Cred> {
    let (code, state) = split_code(pasted);
    let state = if state.is_empty() { p.state.clone() } else { state };
    let body = json!({
        "grant_type": "authorization_code",
        "code": code,
        "state": state,
        "client_id": CLIENT_ID,
        "redirect_uri": REDIRECT,
        "code_verifier": p.verifier,
    });
    let cred = token(http, &body).await?;
    Ok(cred)
}

pub async fn refresh(http: &Client, cred: &Cred) -> Result<Cred> {
    let body = json!({ "grant_type": "refresh_token", "refresh_token": cred.refresh, "client_id": CLIENT_ID });
    let mut renewed = token(http, &body).await?;
    if renewed.refresh.is_empty() {
        renewed.refresh.clone_from(&cred.refresh);
    }
    renewed.account = if renewed.account.is_empty() { cred.account.clone() } else { renewed.account };
    Ok(renewed)
}

async fn token(http: &Client, body: &Value) -> Result<Cred> {
    let res = http
        .post(TOKEN)
        .header("content-type", "application/json")
        .json(body)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .context("connexion au serveur de jetons Claude")?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("Claude {status} : {}", oauth_error(&text));
    }
    let v: Value = serde_json::from_str(&text).context("réponse de jeton illisible")?;
    let access = v["access_token"].as_str().unwrap_or_default().to_owned();
    anyhow::ensure!(!access.is_empty(), "réponse de jeton sans access_token");
    let expires_in = v["expires_in"].as_u64().unwrap_or(3600);
    Ok(Cred {
        access,
        refresh: v["refresh_token"].as_str().unwrap_or_default().to_owned(),
        expires_at: chrono::Utc::now().timestamp() + i64::try_from(expires_in).unwrap_or(3600),
        account: v["subscriptionType"].as_str().unwrap_or("claude").to_owned(),
        project: String::new(),
    })
}

/// Imports the session the official Claude Code CLI already logged in (`<dir>/.credentials.json`).
pub fn import_cli(dir: &std::path::Path) -> Option<Cred> {
    let raw = std::fs::read_to_string(dir.join(".credentials.json")).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let oauth = &v["claudeAiOauth"];
    let access = oauth["accessToken"].as_str()?.to_owned();
    Some(Cred {
        access,
        refresh: oauth["refreshToken"].as_str().unwrap_or_default().to_owned(),
        expires_at: oauth["expiresAt"].as_i64().map_or(0, |ms| ms / 1000),
        account: oauth["subscriptionType"].as_str().unwrap_or("claude-code").to_owned(),
        project: String::new(),
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
    [("five_hour", "5 h"), ("seven_day", "7 jours"), ("seven_day_opus", "7 j Opus"), ("seven_day_sonnet", "7 j Sonnet")]
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
        assert!(url.starts_with("https://claude.ai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e"));
        for needle in ["response_type=code", "code_challenge_method=S256", "redirect_uri=https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fcode%2Fcallback"] {
            assert!(url.contains(needle), "manque {needle}");
        }
        assert!(url.contains(&format!("code_challenge={}", p.challenge)));
        assert!(url.contains(&format!("state={}", p.state)));
        assert!(url.contains("scope=org%3Acreate_api_key%20user%3Aprofile%20user%3Ainference"));
    }

    #[test]
    fn pasted_code_may_carry_its_state() {
        assert_eq!(split_code("abc#def"), ("abc".into(), "def".into()));
        assert_eq!(split_code(" abc \n"), ("abc".into(), String::new()));
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
        assert!(import_cli(&dir.join("nope")).is_none());
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
