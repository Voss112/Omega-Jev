//! GitHub account for the agent's `git` and `gh`: OAuth device flow (public GitHub CLI client) or a pasted token.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::Value;

use super::oauth_error;
use crate::domain::auth::Cred;

/// Public OAuth client of the official GitHub CLI (device flow enabled, no secret).
pub const CLIENT_ID: &str = "178c6fc778ccc68e1d6a";
pub const DEVICE: &str = "https://github.com/login/device/code";
pub const TOKEN: &str = "https://github.com/login/oauth/access_token";
pub const USER: &str = "https://api.github.com/user";
/// Page where the user types the device code.
pub const VERIFY: &str = "https://github.com/login/device";
pub const SCOPE: &str = "repo read:org gist workflow";

/// What the user types on github.com/login/device.
#[derive(Clone, Debug)]
pub struct Device {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

pub async fn start(http: &Client) -> Result<Device> {
    let res = http
        .post(DEVICE)
        .header("accept", "application/json")
        .form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("connexion à GitHub")?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("GitHub {status} : {}", oauth_error(&text));
    }
    device(&serde_json::from_str(&text).context("réponse GitHub illisible")?)
}

fn device(v: &Value) -> Result<Device> {
    let field = |k: &str| v[k].as_str().map(str::to_owned).with_context(|| format!("réponse GitHub sans {k}"));
    Ok(Device {
        device_code: field("device_code")?,
        user_code: field("user_code")?,
        verification_uri: field("verification_uri")?,
        interval: v["interval"].as_u64().unwrap_or(5).max(1),
        expires_in: v["expires_in"].as_u64().unwrap_or(900),
    })
}

/// Polls until the user approves the code on github.com (or the code expires).
pub async fn wait(http: &Client, device: &Device) -> Result<Cred> {
    let mut interval = device.interval;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(device.expires_in);
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let res = http
            .post(TOKEN)
            .header("accept", "application/json")
            .form(&[
                ("client_id", CLIENT_ID),
                ("device_code", device.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .context("connexion à GitHub")?;
        let v: Value = res.json().await.unwrap_or(Value::Null);
        match poll(&v) {
            Poll::Token(token) => return import_token(http, &token).await,
            Poll::Pending => {}
            Poll::SlowDown => interval += 5,
            Poll::Failed(message) => bail!("GitHub : {message}"),
        }
    }
    bail!("code GitHub expiré — recommencez")
}

#[derive(Debug, PartialEq, Eq)]
enum Poll {
    Token(String),
    Pending,
    SlowDown,
    Failed(String),
}

fn poll(v: &Value) -> Poll {
    if let Some(token) = v["access_token"].as_str().filter(|t| !t.is_empty()) {
        return Poll::Token(token.to_owned());
    }
    match v["error"].as_str() {
        Some("authorization_pending") => Poll::Pending,
        Some("slow_down") => Poll::SlowDown,
        Some("access_denied") => Poll::Failed("accès refusé".to_owned()),
        Some("expired_token") => Poll::Failed("code expiré — recommencez".to_owned()),
        _ => Poll::Failed(oauth_error(&v.to_string())),
    }
}

/// A token (device flow or pasted `ghp_…`/`github_pat_…`), checked against `/user` and labelled with the login.
pub async fn import_token(http: &Client, token: &str) -> Result<Cred> {
    let token = token.trim();
    anyhow::ensure!(!token.is_empty(), "jeton vide");
    let res = http
        .get(USER)
        .bearer_auth(token)
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "omega")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .context("connexion à api.github.com")?;
    let status = res.status();
    let v: Value = res.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("GitHub {status} : jeton refusé");
    }
    let login = v["login"].as_str().unwrap_or("github").to_owned();
    // Device-flow and classic tokens do not expire; a fine-grained token's expiry is enforced by GitHub itself.
    Ok(Cred { access: token.to_owned(), account: login, expires_at: i64::MAX, ..Cred::default() })
}

/// Environment that authenticates `git` (HTTPS to github.com) and `gh` for one child process, nothing on disk.
pub fn shell_env(token: &str) -> Vec<(String, String)> {
    use base64::Engine as _;
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    vec![
        ("GH_TOKEN".to_owned(), token.to_owned()),
        ("GITHUB_TOKEN".to_owned(), token.to_owned()),
        ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
        ("GIT_CONFIG_COUNT".to_owned(), "1".to_owned()),
        ("GIT_CONFIG_KEY_0".to_owned(), "http.https://github.com/.extraheader".to_owned()),
        ("GIT_CONFIG_VALUE_0".to_owned(), format!("AUTHORIZATION: basic {basic}")),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn device_codes_are_read_with_defaults() {
        let d = device(&json!({ "device_code": "dc", "user_code": "ABCD-1234", "verification_uri": "https://github.com/login/device" }))
            .unwrap();
        assert_eq!((d.user_code.as_str(), d.interval, d.expires_in), ("ABCD-1234", 5, 900));
        assert!(device(&json!({ "user_code": "x" })).is_err());
    }

    #[test]
    fn polling_states_are_distinguished() {
        assert_eq!(poll(&json!({ "access_token": "gho_x" })), Poll::Token("gho_x".into()));
        assert_eq!(poll(&json!({ "error": "authorization_pending" })), Poll::Pending);
        assert_eq!(poll(&json!({ "error": "slow_down" })), Poll::SlowDown);
        assert!(matches!(poll(&json!({ "error": "access_denied" })), Poll::Failed(_)));
    }

    #[test]
    fn shell_env_authenticates_git_without_writing_config() {
        let env = shell_env("gho_secret");
        let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("GH_TOKEN"), Some("gho_secret"));
        assert_eq!(get("GIT_CONFIG_KEY_0"), Some("http.https://github.com/.extraheader"));
        let header = get("GIT_CONFIG_VALUE_0").unwrap();
        assert!(header.starts_with("AUTHORIZATION: basic ") && !header.contains("gho_secret"), "jeton encodé");
    }
}
