//! Gemini consumer subscription (Google AI Pro/Ultra → Code Assist), OAuth + long-running onboarding.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::{Value, json};

use super::{oauth_error, pkce};
use crate::domain::auth::Cred;

/// Installed-application client of Gemini CLI (public, per Google's OAuth docs).
pub const CLIENT_ID: &str = "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
pub const CLIENT_SECRET: &str = "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";
pub const AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN: &str = "https://oauth2.googleapis.com/token";
pub const USERINFO: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
pub const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile";
pub const CODE_ASSIST: &str = "https://cloudcode-pa.googleapis.com/v1internal";

pub fn authorize_url(p: &pkce::Pkce, redirect: &str) -> String {
    format!(
        "{AUTHORIZE}?client_id={CLIENT_ID}&response_type=code&redirect_uri={}&scope={}&access_type=offline&prompt=consent&include_granted_scopes=true&code_challenge={}&code_challenge_method=S256&state={}",
        pkce::encode(redirect),
        pkce::encode(SCOPES),
        p.challenge,
        p.state
    )
}

pub async fn exchange(http: &Client, code: &str, verifier: &str, redirect: &str) -> Result<Cred> {
    let form = [
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect),
    ];
    let cred = token(http, &form).await?;
    let account = email(http, &cred.access).await.unwrap_or_else(|| "google".to_owned());
    Ok(Cred { account, ..cred })
}

pub async fn refresh(http: &Client, cred: &Cred) -> Result<Cred> {
    let form = [("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET), ("grant_type", "refresh_token"), ("refresh_token", cred.refresh.as_str())];
    let mut renewed = token(http, &form).await?;
    if renewed.refresh.is_empty() {
        renewed.refresh.clone_from(&cred.refresh);
    }
    renewed.account = if renewed.account.is_empty() { cred.account.clone() } else { renewed.account };
    renewed.project.clone_from(&cred.project);
    Ok(renewed)
}

async fn token(http: &Client, form: &[(&str, &str)]) -> Result<Cred> {
    let res = http
        .post(TOKEN)
        .form(form)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("connexion au serveur de jetons Google")?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Google {status} : {}", oauth_error(&text));
    }
    let v: Value = serde_json::from_str(&text).context("réponse de jeton illisible")?;
    let access = v["access_token"].as_str().unwrap_or_default().to_owned();
    anyhow::ensure!(!access.is_empty(), "réponse de jeton sans access_token");
    let expires_in = v["expires_in"].as_u64().unwrap_or(3600);
    Ok(Cred {
        access,
        refresh: v["refresh_token"].as_str().unwrap_or_default().to_owned(),
        expires_at: chrono::Utc::now().timestamp() + i64::try_from(expires_in).unwrap_or(3600),
        account: String::new(),
        project: String::new(),
    })
}

async fn email(http: &Client, access: &str) -> Option<String> {
    let res = http.get(USERINFO).bearer_auth(access).timeout(Duration::from_secs(8)).send().await.ok()?;
    let v: Value = res.json().await.ok()?;
    v["email"].as_str().map(str::to_owned)
}

/// `(tier id, Code Assist project)`: `loadCodeAssist`, then `onboardUser` when no project is attached.
pub async fn tier(http: &Client, access: &str, project: &str) -> Result<(String, String)> {
    let probe = json!({ "cloudaicompanionProject": project, "metadata": { "pluginType": "GEMINI" } });
    let v = post(http, access, "loadCodeAssist", &probe).await?;
    if let Some((tier, found)) = tier_of(&v)
        && !found.is_empty()
    {
        return Ok((tier, found));
    }
    let tier_id = v["currentTier"]["id"].as_str().unwrap_or("free-tier").to_owned();
    let onboard = json!({ "tierId": tier_id, "metadata": { "pluginType": "GEMINI" } });
    let op = post(http, access, "onboardUser", &onboard).await?;
    let Some(name) = op["name"].as_str().filter(|n| !n.is_empty()) else {
        return Ok((tier_id, String::new()));
    };
    anyhow::ensure!(name.chars().all(|c| c.is_ascii_alphanumeric() || "/_-".contains(c)), "nom d'opération inattendu");
    for _ in 0..15 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let res = http
            .get(format!("{CODE_ASSIST}/{name}"))
            .bearer_auth(access)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .context("suivi de l'activation Code Assist")?;
        let v: Value = res.json().await.unwrap_or(Value::Null);
        if v["done"].as_bool() == Some(true) {
            let project = v["response"]["cloudaicompanionProject"].as_str().unwrap_or_default().to_owned();
            return Ok((tier_id, project));
        }
    }
    Ok((tier_id, String::new()))
}

pub fn tier_of(v: &Value) -> Option<(String, String)> {
    let tier = v["currentTier"]["id"].as_str()?.to_owned();
    Some((tier, v["cloudaicompanionProject"].as_str().unwrap_or_default().to_owned()))
}

async fn post(http: &Client, access: &str, method: &str, body: &Value) -> Result<Value> {
    let res = http
        .post(format!("{CODE_ASSIST}:{method}"))
        .bearer_auth(access)
        .json(body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .with_context(|| format!("Code Assist {method}"))?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Code Assist {method} {status} : {}", oauth_error(&text));
    }
    serde_json::from_str(&text).with_context(|| format!("réponse {method} illisible"))
}

/// Envelope expected by every `v1internal` call.
pub fn envelope(project: &str, model: &str, request: Value) -> Value {
    json!({ "model": model, "project": project, "request": request })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_matches_the_desktop_client_flow() {
        let p = pkce::Pkce::new();
        let url = authorize_url(&p, "http://127.0.0.1:41234/oauth2callback");
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        for needle in [
            "client_id=681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com",
            "response_type=code",
            "access_type=offline",
            "prompt=consent",
            "code_challenge_method=S256",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A41234%2Foauth2callback",
            "scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcloud-platform",
        ] {
            assert!(url.contains(needle), "manque {needle}");
        }
        assert!(url.contains(&format!("state={}", p.state)));
    }

    #[test]
    fn request_envelope_wraps_model_and_project() {
        let body = envelope("proj-1", "gemini-2.5-pro", json!({ "contents": [] }));
        assert_eq!(body["model"], "gemini-2.5-pro");
        assert_eq!(body["project"], "proj-1");
        assert!(body.get("request").is_some());
    }

    #[test]
    fn tier_is_read_from_load_code_assist() {
        let v = json!({ "currentTier": { "id": "standard" }, "cloudaicompanionProject": "proj-9" });
        assert_eq!(tier_of(&v), Some(("standard".to_owned(), "proj-9".to_owned())));
        assert!(tier_of(&json!({})).is_none());
    }
}
