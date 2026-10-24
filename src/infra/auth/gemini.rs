//! Gemini consumer subscription (Google AI Pro/Ultra → Code Assist), OAuth + long-running onboarding.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::{Value, json};

use super::{oauth_error, pkce};
use crate::domain::auth::{Cred, Window};

/// Installed-application client of Gemini CLI (public, per Google's OAuth docs).
pub const CLIENT_ID: &str = "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
pub const CLIENT_SECRET: &str = "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";
pub const AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN: &str = "https://oauth2.googleapis.com/token";
pub const USERINFO: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
pub const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile";
pub const CODE_ASSIST: &str = "https://cloudcode-pa.googleapis.com/v1internal";
/// Code Assist serves the official CLI; requests identify as it.
pub const USER_AGENT: &str = "GeminiCLI/0.20.0 (win32; x64)";

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

/// Metadata every Code Assist call expects; `duetProject` only when a project is known.
fn metadata(project: &str) -> Value {
    let mut m = json!({ "pluginType": "GEMINI", "ideType": "IDE_UNSPECIFIED", "platform": "PLATFORM_UNSPECIFIED" });
    if !project.is_empty() {
        m["duetProject"] = json!(project);
    }
    m
}

/// Project id from either shape the API uses: a string (`loadCodeAssist`) or `{id}` (onboarding operation).
fn project_of(v: &Value) -> String {
    let Some(project) = v.get("cloudaicompanionProject") else {
        return String::new();
    };
    project.as_str().or_else(|| project["id"].as_str()).unwrap_or_default().to_owned()
}

/// `(tier id, requires a user project)`; mirrors the official CLI's default-tier selection.
fn onboard_tier(v: &Value) -> (String, bool) {
    if let Some(tier) = v["allowedTiers"].as_array().and_then(|tiers| tiers.iter().find(|t| t["isDefault"].as_bool() == Some(true))) {
        let id = tier["id"].as_str().unwrap_or_default().to_owned();
        let needs_project = tier["userDefinedCloudaicompanionProject"].as_bool() == Some(true);
        return (id, needs_project);
    }
    ("legacy-tier".to_owned(), true)
}

/// Reason Google gives for an account that cannot use Code Assist, with its validation link when present.
fn ineligible(v: &Value) -> Option<String> {
    let tiers = v["ineligibleTiers"].as_array()?;
    let tier = tiers.iter().find(|t| t["validationUrl"].as_str().is_some_and(|u| !u.is_empty())).or_else(|| tiers.first())?;
    let reason = tier["reasonMessage"].as_str().filter(|s| !s.is_empty()).unwrap_or("compte inéligible à Gemini Code Assist");
    Some(match tier["validationUrl"].as_str().filter(|u| !u.is_empty()) {
        Some(url) => format!("{reason} — validez ce compte : {url}"),
        None => reason.to_owned(),
    })
}

/// The individual tier refused to this client (Google moved personal accounts to Antigravity).
fn unsupported(v: &Value) -> Option<String> {
    v["ineligibleTiers"].as_array()?.iter().find(|t| t["reasonCode"].as_str() == Some("UNSUPPORTED_CLIENT")).map(|t| {
        let google = t["reasonMessage"].as_str().unwrap_or_default();
        format!(
            "Google n'ouvre plus Gemini Code Assist aux comptes personnels (Google AI Pro/Ultra) : « {google} ». \n             Utilisez une clé API Gemini (aistudio.google.com) en mode « Clé API », ou un projet Google Cloud (GOOGLE_CLOUD_PROJECT)."
        )
    })
}

/// Google sometimes blocks Code Assist until the account is validated (age, region, Workspace policy).
fn validation_url(v: &Value) -> Option<String> {
    v["ineligibleTiers"].as_array()?.iter().find_map(|tier| {
        if tier["reasonCode"].as_str() != Some("VALIDATION_REQUIRED") {
            return None;
        }
        tier["validationUrl"].as_str().filter(|u| !u.is_empty()).map(str::to_owned)
    })
}

/// A Workspace/standard account may supply its project through the environment, like the official CLI.
pub fn env_project() -> String {
    ["GOOGLE_CLOUD_PROJECT", "GOOGLE_CLOUD_PROJECT_ID"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .map(|v| v.trim().to_owned())
        .unwrap_or_default()
}

/// `(tier id, Code Assist project)`: mirrors the official CLI — `loadCodeAssist`, then `onboardUser`
/// only when no tier is attached. Onboarding returns its project as `{id}`, not as a string.
pub async fn tier(http: &Client, access: &str, project: &str) -> Result<(String, String)> {
    let project = if project.trim().is_empty() { env_project() } else { project.trim().to_owned() };
    let mut probe = json!({ "metadata": metadata(&project) });
    if !project.is_empty() {
        probe["cloudaicompanionProject"] = json!(project);
    }
    let v = post(http, access, "loadCodeAssist", &probe).await.map_err(license_hint)?;

    if let Some((tier, found)) = tier_of(&v) {
        if !found.is_empty() {
            return Ok((tier, found));
        }
        if !project.is_empty() {
            return Ok((tier, project.clone()));
        }
        if let Some(message) = ineligible(&v) {
            bail!("{message}");
        }
        bail!(
            "Compte Gemini sans projet Code Assist : définissez GOOGLE_CLOUD_PROJECT, \
             ou passez Gemini en « Clé API » dans ⚙ Paramètres."
        );
    }

    // Before onboarding, a validation link is the only actionable answer.
    if let Some(url) = validation_url(&v) {
        bail!("Google demande de valider ce compte avant Code Assist : {url} — validez, puis reconnectez-vous.");
    }
    let has_allowed = v["allowedTiers"].as_array().is_some();
    let (tier_id, needs_project) = onboard_tier(&v);
    if !has_allowed && project.is_empty() && let Some(message) = ineligible(&v) {
        bail!("{message}");
    }
    let free = tier_id == "free-tier";
    // Personal accounts (Google AI Pro/Ultra included) lost Code Assist: say so instead of asking for a project.
    if !free && needs_project && project.is_empty() && let Some(reason) = unsupported(&v) {
        bail!("{reason}");
    }
    if !free && needs_project && project.is_empty() {
        bail!("Cette offre Gemini exige un projet Google Cloud : définissez GOOGLE_CLOUD_PROJECT, puis reconnectez-vous.");
    }

    // The free tier manages its own project; sending one triggers "Precondition Failed".
    let mut onboard = json!({ "tierId": tier_id, "metadata": if free { metadata("") } else { metadata(&project) } });
    if !free && !project.is_empty() {
        onboard["cloudaicompanionProject"] = json!(project);
    }
    let op = post(http, access, "onboardUser", &onboard).await.map_err(license_hint)?;
    if op["done"].as_bool() == Some(true) {
        let found = project_of(&op["response"]);
        if !found.is_empty() {
            return Ok((tier_id, found));
        }
        if !project.is_empty() {
            return Ok((tier_id, project));
        }
        bail!("Code Assist : activation terminée sans projet. Réessayez, ou passez Gemini en « Clé API ».");
    }
    let Some(name) = op["name"].as_str().filter(|n| !n.is_empty()) else {
        if !project.is_empty() {
            return Ok((tier_id, project));
        }
        bail!("Code Assist : aucune opération d'activation renvoyée. Réessayez dans une minute.");
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
            let found = project_of(&v["response"]);
            if !found.is_empty() {
                return Ok((tier_id, found));
            }
            if !project.is_empty() {
                return Ok((tier_id, project));
            }
            bail!("Code Assist : activation terminée sans projet. Réessayez, ou passez Gemini en « Clé API ».");
        }
    }
    bail!("Code Assist : activation toujours en cours (30 s). Réessayez dans une minute.")
}

/// #3501: the Google account has no Gemini Code Assist licence — say what to do instead of quoting the API.
fn license_hint(err: anyhow::Error) -> anyhow::Error {
    let text = format!("{err:#}");
    match hint(&text) {
        Some(message) => anyhow::anyhow!("{message}"),
        None => err,
    }
}

/// The actionable rewrite of a Code Assist licence error, if the body is one.
pub fn hint(body: &str) -> Option<String> {
    (body.contains("3501") || body.contains("valid license")).then(|| {
        "Compte Google sans licence Gemini Code Assist (403 #3501). Activez Code Assist sur ce compte \
         (codeassist.google.com), puis reconnectez-vous — ou passez Gemini en « Clé API » dans ⚙ Paramètres."
            .to_owned()
    })
}

pub fn tier_of(v: &Value) -> Option<(String, String)> {
    let tier = v["currentTier"]["id"].as_str()?.to_owned();
    Some((tier, project_of(v)))
}

async fn post(http: &Client, access: &str, method: &str, body: &Value) -> Result<Value> {
    let res = http
        .post(format!("{CODE_ASSIST}:{method}"))
        .bearer_auth(access)
        .header("user-agent", USER_AGENT)
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

/// Plan quota per model (`retrieveUserQuota`), best-effort: `None` hides the section.
pub async fn usage(http: &Client, access: &str, project: &str) -> Option<Vec<Window>> {
    let v = post(http, access, "retrieveUserQuota", &json!({ "project": project })).await.ok()?;
    Some(quota_windows(&v)).filter(|w| !w.is_empty())
}

/// One row per model bucket: used fraction = 1 − `remainingFraction`.
pub fn quota_windows(v: &Value) -> Vec<Window> {
    v["buckets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| {
            let remaining = b["remainingFraction"].as_f64()?;
            Some(Window {
                label: b["modelId"].as_str().unwrap_or("gemini").to_owned(),
                fraction: (1.0 - remaining).clamp(0.0, 1.0) as f32,
                resets: b["resetTime"].as_str().map(str::to_owned),
            })
        })
        .collect()
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
    fn a_personal_account_refused_by_code_assist_is_explained() {
        let v = json!({
            "allowedTiers": [{ "id": "standard-tier", "isDefault": true, "userDefinedCloudaicompanionProject": true }],
            "ineligibleTiers": [{ "reasonCode": "UNSUPPORTED_CLIENT", "reasonMessage": "migrate to Antigravity", "tierId": "free-tier" }]
        });
        let reason = unsupported(&v).unwrap();
        assert!(reason.contains("migrate to Antigravity") && reason.contains("Clé API"));
        assert!(unsupported(&json!({})).is_none());
    }

    #[test]
    fn quota_buckets_become_usage_rows() {
        let v = json!({ "buckets": [
            { "modelId": "gemini-3.8-flash", "remainingFraction": 0.0, "resetTime": "2026-10-25T00:00:00Z" },
            { "modelId": "gemini-3.8-pro", "remainingFraction": 0.75 },
            { "modelId": "broken" }
        ] });
        let w = quota_windows(&v);
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].label.as_str(), w[0].fraction), ("gemini-3.8-flash", 1.0));
        assert!((w[1].fraction - 0.25).abs() < 1e-6);
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

    #[test]
    fn a_missing_licence_becomes_an_actionable_message() {
        let err = license_hint(anyhow::anyhow!("Code Assist loadCodeAssist 403 Forbidden : (#3501) you do not have a valid license"));
        assert!(format!("{err:#}").contains("Clé API"));
        let other = license_hint(anyhow::anyhow!("timeout"));
        assert!(format!("{other:#}").contains("timeout"));
    }

    #[test]
    fn code_assist_metadata_names_the_ide_plugin() {
        let m = metadata("");
        assert_eq!((m["pluginType"].as_str(), m["ideType"].as_str()), (Some("GEMINI"), Some("IDE_UNSPECIFIED")));
        assert!(m.get("duetProject").is_none());
        assert_eq!(metadata("proj-1")["duetProject"], "proj-1");
    }

    #[test]
    fn an_onboarded_project_is_read_from_its_object() {
        let op = json!({ "response": { "cloudaicompanionProject": { "id": "proj-7", "name": "Code Assist" } } });
        assert_eq!(project_of(&op["response"]), "proj-7");
        assert_eq!(project_of(&json!({ "cloudaicompanionProject": "proj-8" })), "proj-8");
        assert_eq!(project_of(&json!({})), "");
    }

    #[test]
    fn the_default_allowed_tier_drives_onboarding() {
        let v = json!({ "allowedTiers": [
            { "id": "free-tier", "isDefault": false },
            { "id": "standard-tier", "isDefault": true, "userDefinedCloudaicompanionProject": true }
        ] });
        assert_eq!(onboard_tier(&v), ("standard-tier".to_owned(), true));
        assert_eq!(onboard_tier(&json!({})), ("legacy-tier".to_owned(), true));
    }

    #[test]
    fn an_ineligible_account_yields_its_reason_and_validation_url() {
        let v = json!({ "ineligibleTiers": [{
            "reasonCode": "VALIDATION_REQUIRED",
            "reasonMessage": "validation requise",
            "validationUrl": "https://example.test/verify"
        }] });
        assert_eq!(validation_url(&v).as_deref(), Some("https://example.test/verify"));
        let message = ineligible(&v).unwrap();
        assert!(message.contains("validation requise") && message.contains("https://example.test/verify"));
    }
}
