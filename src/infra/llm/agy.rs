//! Gemini through the Antigravity CLI (`agy`): since June 2026 the only door Google leaves open to personal
//! Google AI Pro/Ultra plans. Text only — Omega's tools never reach it — run headless in an empty folder, where
//! shell commands are refused by default, with the conversation flattened into one prompt.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::OnceLock,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use super::{Completion, Delta};
use crate::{domain::engine::Effort, infra::process};

/// Official installer (Google), run in a visible terminal at the user's request.
pub const INSTALL: &str = "irm https://antigravity.google/cli/install.ps1 | iex";
const TIMEOUT: Duration = Duration::from_secs(600);

/// `agy` on PATH, or where the official installer puts it. Asked every frame by the settings form and the
/// panel: the PATH scan is reused for a few seconds.
pub fn binary() -> Option<PathBuf> {
    type Found = Option<(std::time::Instant, Option<PathBuf>)>;
    static CACHE: std::sync::Mutex<Found> = std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((at, found)) = cache.as_ref()
        && at.elapsed() < Duration::from_secs(5)
    {
        return found.clone();
    }
    let found = find();
    *cache = Some((std::time::Instant::now(), found.clone()));
    found
}

fn find() -> Option<PathBuf> {
    process::which("agy").or_else(|| {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
        Some(local.join("agy").join("bin").join("agy.exe")).filter(|p| p.is_file())
    })
}

/// Runs Google's installer without a window (it only downloads and checks `agy.exe`), then finds the binary.
pub async fn install() -> Result<PathBuf> {
    if let Some(bin) = find() {
        return Ok(bin);
    }
    let mut cmd = process::shell(INSTALL);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = tokio::time::timeout(Duration::from_secs(300), cmd.output())
        .await
        .context("installation d'Antigravity : délai de 5 min dépassé")?
        .context("installation d'Antigravity")?;
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    find().with_context(|| format!("agy introuvable après l'installation : {}", log.trim().chars().take(400).collect::<String>()))
}

/// A visible console running `agy`: first run opens the Google sign-in page; `/quit` closes the session.
pub fn open_login(bin: &Path) -> Result<()> {
    let script = format!(
        "Write-Host 'Antigravity : connectez-vous dans la page Google, puis tapez /quit et fermez cette fenetre.'; & '{}'",
        bin.display().to_string().replace('\'', "''")
    );
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoLogo", "-NoProfile", "-NoExit", "-Command", &script]);
    #[cfg(windows)]
    {
        // A console of its own: Omega is a GUI process and has none to share.
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        std::os::windows::process::CommandExt::creation_flags(&mut cmd, CREATE_NEW_CONSOLE);
    }
    cmd.spawn().context("ouverture de la console Antigravity")?;
    Ok(())
}

/// The conversation as one prompt: agy keeps its own agent prompt, Omega's instructions travel as text.
pub fn prompt(messages: &[Value]) -> String {
    let mut out = String::from(
        "Answer the final USER message directly, as plain text. Do not use any tool, file or command: \
         everything you need is below.\n\n",
    );
    for m in messages {
        let role = m["role"].as_str().unwrap_or_default();
        let text = match &m["content"] {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
            _ => String::new(),
        };
        if text.trim().is_empty() {
            continue;
        }
        let label = match role {
            "system" => "INSTRUCTIONS",
            "assistant" => "ASSISTANT",
            "tool" => "TOOL RESULT",
            _ => "USER",
        };
        out.push_str(&format!("{label}:\n{text}\n\n"));
    }
    out
}

/// The `agy models` slug for `wanted` at `effort` (`gemini-3.8-flash` → `gemini-3.8-flash-medium`), if listed.
pub fn slug(models: &[String], wanted: &str, effort: Effort) -> Option<String> {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return None;
    }
    if models.iter().any(|m| m == wanted) {
        return Some(wanted.to_owned());
    }
    let level = match effort {
        Effort::Off | Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High | Effort::Max => "high",
    };
    let family: Vec<&String> = models.iter().filter(|m| m.starts_with(&format!("{wanted}-"))).collect();
    family.iter().find(|m| m.ends_with(&format!("-{level}"))).or_else(|| family.first()).map(|m| (*m).clone())
}

/// Model slugs `agy models` prints (cached for the session).
pub async fn models(bin: &Path) -> Result<Vec<String>> {
    static CACHE: OnceLock<Vec<String>> = OnceLock::new();
    if let Some(list) = CACHE.get() {
        return Ok(list.clone());
    }
    let out = tokio::time::timeout(Duration::from_secs(60), process::command(bin).arg("models").stdin(Stdio::null()).output())
        .await
        .context("agy models : délai dépassé")?
        .context("agy introuvable")?;
    let text = String::from_utf8_lossy(&out.stdout);
    anyhow::ensure!(
        out.status.success(),
        "agy models a échoué ({}) — lancez `agy` une fois dans un terminal pour vous connecter",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    // Not signed in: agy says so and still exits 0.
    if format!("{text}{}", String::from_utf8_lossy(&out.stderr)).to_lowercase().contains("sign in") {
        bail!("Antigravity : connexion requise — ⚙ Paramètres → Gemini → « Se connecter à Antigravity »");
    }
    let list = parse_models(&text);
    if !list.is_empty() {
        let _ = CACHE.set(list.clone());
    }
    Ok(list)
}

fn parse_models(text: &str) -> Vec<String> {
    let mut list: Vec<String> = text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '|' | '"' | '`' | '(' | ')' | '[' | ']'))
        .filter(|w| w.starts_with("gemini") && w.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.'))
        .map(str::to_owned)
        .collect();
    list.dedup();
    list
}

/// agy's own sign-in, from the Windows credential store: access token, its expiry and the account e-mail.
pub struct Session {
    pub access: String,
    pub expiry: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub email: String,
}

/// The session agy saved (`gemini:antigravity` in the Windows credential store), if signed in.
pub fn session() -> Option<Session> {
    let v: Value = serde_json::from_str(&keyring::read("gemini:antigravity")?).ok()?;
    let access = v["token"]["access_token"].as_str().filter(|t| !t.is_empty())?.to_owned();
    let expiry = v["token"]["expiry"].as_str().and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok());
    Some(Session { access, expiry, email: v["id_token_email"].as_str().unwrap_or_default().to_owned() })
}

/// Plan quota of the Antigravity sign-in, one row per shared pool. agy refreshes its own token on any run,
/// so an expired one is renewed by a cheap `agy models` first.
pub async fn usage(http: &reqwest::Client, bin: &Path) -> Option<Vec<crate::domain::auth::Window>> {
    let fresh = |s: &Session| s.expiry.is_none_or(|e| e > chrono::Utc::now() + chrono::Duration::seconds(60));
    let session = match session() {
        Some(s) if fresh(&s) => s,
        _ => {
            let renew = process::command(bin).arg("models").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
            let _ = tokio::time::timeout(Duration::from_secs(60), renew).await;
            session().filter(fresh)?
        }
    };
    let res = http
        .post(format!("{}:retrieveUserQuota", crate::infra::auth::gemini::CODE_ASSIST))
        .bearer_auth(&session.access)
        // Code Assist only answers Antigravity tokens that present themselves as Antigravity.
        .header("user-agent", "antigravity")
        .json(&json!({}))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .ok()?;
    if !res.status().is_success() {
        return None;
    }
    let v: Value = res.json().await.ok()?;
    Some(pools(&v)).filter(|w| !w.is_empty())
}

/// Models of one plan share an allowance: buckets with the same remaining fraction and reset time become one
/// row named after their families (`Gemini`, `Claude · GPT`). Buckets without a reset (autocomplete) are skipped.
fn pools(v: &Value) -> Vec<crate::domain::auth::Window> {
    let mut rows: Vec<(String, f64, Vec<String>)> = Vec::new();
    for b in v["buckets"].as_array().into_iter().flatten() {
        let (Some(id), Some(left), Some(reset)) = (b["modelId"].as_str(), b["remainingFraction"].as_f64(), b["resetTime"].as_str())
        else {
            continue;
        };
        let family = match id.split('-').next().unwrap_or(id) {
            "gpt" => "GPT".to_owned(),
            f => {
                // Char-wise: a byte slice would panic on an empty or non-ASCII id sent by the server.
                let mut chars = f.chars();
                chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_else(|| id.to_owned())
            }
        };
        match rows.iter_mut().find(|(r, l, _)| r == reset && (l - left).abs() < 1e-6) {
            Some((_, _, families)) if !families.contains(&family) => families.push(family),
            Some(_) => {}
            None => rows.push((reset.to_owned(), left, vec![family])),
        }
    }
    rows.into_iter()
        .map(|(reset, left, families)| crate::domain::auth::Window {
            label: families.join(" · "),
            fraction: (1.0 - left).clamp(0.0, 1.0) as f32,
            resets: Some(reset),
        })
        .collect()
}

#[cfg(windows)]
mod keyring {
    use windows_sys::Win32::Security::Credentials::{CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW};

    /// Secret of a generic credential, as UTF-8.
    pub fn read(target: &str) -> Option<String> {
        let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated; on success `cred` points to a block owned by the OS until `CredFree`.
        unsafe {
            if CredReadW(wide.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) == 0 || cred.is_null() {
                return None;
            }
            let c = &*cred;
            let blob = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
            let text = String::from_utf8(blob.to_vec()).ok();
            CredFree(cred.cast());
            text
        }
    }
}

#[cfg(not(windows))]
mod keyring {
    pub fn read(_target: &str) -> Option<String> {
        None
    }
}

/// One headless run: the prompt goes in on stdin (no command-line length limit), text deltas stream out.
pub async fn run(bin: &Path, model: &str, effort: Effort, messages: &[Value], mut on: impl FnMut(Delta<'_>)) -> Result<Completion> {
    // An empty folder of its own: agy's file tools have nothing of the user's to read or change.
    let dir = std::env::temp_dir().join("omega-agy");
    std::fs::create_dir_all(&dir).context("dossier de travail agy")?;
    let mut cmd = process::command(bin);
    // stream-json input: the prompt comes from stdin only, `-p` would swallow the next flag.
    cmd.args(["--input-format", "stream-json", "--output-format", "stream-json", "--disable-slash-commands"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(slug) = slug(&models(bin).await.unwrap_or_default(), model, effort) {
        cmd.args(["--model", &slug]);
    }
    let mut child = cmd.spawn().context("démarrage de agy")?;
    let line = json!({ "event": "user", "message": { "content": prompt(messages) } }).to_string();
    let mut stdin = child.stdin.take().context("stdin agy")?;
    stdin.write_all(line.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    drop(stdin);
    let (stdout, stderr) = (child.stdout.take().context("stdout agy")?, child.stderr.take().context("stderr agy")?);
    // Drained alongside stdout: a full stderr pipe would block agy before it ends its stdout (a deadlock until the
    // timeout). Only the head is kept for the error message.
    let errors = tokio::spawn(async move {
        let mut head = Vec::new();
        let mut stderr = stderr;
        let _ = (&mut stderr).take(16 * 1024).read_to_end(&mut head).await;
        let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
        String::from_utf8_lossy(&head).into_owned()
    });
    let read = async {
        let mut out = Completion::default();
        let mut result = None;
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await? {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            match v["event"].as_str() {
                Some("step_update") => {
                    let step = &v["step_update"];
                    if step["step_type"] == "agent_response"
                        && let Some(delta) = step["text_delta"].as_str().filter(|d| !d.is_empty())
                    {
                        on(Delta::Text(delta));
                        out.content.push_str(delta);
                    }
                }
                Some("result") => result = Some(v["result"].clone()),
                _ => {}
            }
        }
        anyhow::Ok((out, result))
    };
    let (out, result) = tokio::time::timeout(TIMEOUT, read).await.context("agy : délai de 10 min dépassé")??;
    let status = child.wait().await?;
    let err = errors.await.unwrap_or_default();
    let Some(result) = result else {
        bail!("Gemini (agy) sans réponse ({status}) : {}", err.trim().chars().take(400).collect::<String>());
    };
    finish(out, &result)
}

/// The final `result` envelope: status, full text, usage.
fn finish(mut out: Completion, result: &Value) -> Result<Completion> {
    let status = result["status"].as_str().unwrap_or("SUCCESS");
    if status != "SUCCESS" {
        let error = result["error"].as_str().unwrap_or(status);
        // A spent plan reads like any provider's quota error: delegation moves on to the next engine.
        if error.to_lowercase().contains("quota") || error.contains("RESOURCE_EXHAUSTED") {
            bail!("Gemini (agy) : {} — {error}", super::QUOTA);
        }
        bail!("Gemini (agy) {status} : {error}");
    }
    if let Some(text) = result["response"].as_str().filter(|t| !t.is_empty()) {
        out.content = text.to_owned();
    }
    let u = &result["usage"];
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    out.usage.prompt = n("input_tokens") + n("cache_read_tokens");
    out.usage.cached = n("cache_read_tokens");
    out.usage.reasoning = n("thinking_tokens");
    out.usage.completion = n("output_tokens") + n("thinking_tokens");
    out.stop = "stop".to_owned();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_conversation_becomes_one_labelled_prompt() {
        let p = prompt(&[
            json!({ "role": "system", "content": "Be terse." }),
            json!({ "role": "user", "content": "Bonjour ?" }),
            json!({ "role": "assistant", "content": "" }),
            json!({ "role": "user", "content": [{ "type": "text", "text": "Et maintenant ?" }] }),
        ]);
        assert!(p.contains("INSTRUCTIONS:\nBe terse.") && p.contains("USER:\nBonjour ?") && p.contains("USER:\nEt maintenant ?"));
        assert!(!p.contains("ASSISTANT:"), "message vide omis");
    }

    #[test]
    fn model_ids_map_to_agy_slugs() {
        let models = parse_models("Available models:\n  gemini-3.8-flash-low\n  gemini-3.8-flash-medium (default)\n  gemini-3.8-flash-high\n  gemini-3.1-pro-high\n");
        assert_eq!(models.len(), 4);
        assert_eq!(slug(&models, "gemini-3.8-flash", Effort::Medium).as_deref(), Some("gemini-3.8-flash-medium"));
        assert_eq!(slug(&models, "gemini-3.8-flash", Effort::Max).as_deref(), Some("gemini-3.8-flash-high"));
        assert_eq!(slug(&models, "gemini-3.1-pro", Effort::Low).as_deref(), Some("gemini-3.1-pro-high"), "seul niveau listé");
        assert_eq!(slug(&models, "gemini-3.1-pro-high", Effort::Low).as_deref(), Some("gemini-3.1-pro-high"));
        assert_eq!(slug(&models, "gemini-9-ultra", Effort::Low), None, "inconnu : modèle par défaut d'agy");
    }

    #[test]
    fn the_result_envelope_gives_text_usage_and_errors() {
        let ok = json!({ "status": "SUCCESS", "response": "ok", "usage": { "input_tokens": 10, "output_tokens": 2, "thinking_tokens": 3, "cache_read_tokens": 4 } });
        let done = finish(Completion::default(), &ok).unwrap();
        assert_eq!((done.content.as_str(), done.usage.prompt, done.usage.completion, done.usage.cached), ("ok", 14, 5, 4));
        let spent = finish(Completion::default(), &json!({ "status": "ERROR", "error": "Quota exceeded for model" })).err().unwrap();
        assert!(super::super::exhausted(&spent));
        assert!(finish(Completion::default(), &json!({ "status": "ERROR", "error": "unknown model" })).is_err());
    }

    #[test]
    fn quota_buckets_merge_into_shared_pools() {
        let v = json!({ "buckets": [
            { "modelId": "chat_20706", "remainingFraction": 1 },
            { "modelId": "claude-sonnet-4-6", "remainingFraction": 1, "resetTime": "2026-10-24T23:30:55Z" },
            { "modelId": "gemini-2.5-pro", "remainingFraction": 0.75, "resetTime": "2026-10-24T19:58:13Z" },
            { "modelId": "gemini-3.8-flash-tiered", "remainingFraction": 0.75, "resetTime": "2026-10-24T19:58:13Z" },
            { "modelId": "gpt-oss-120b-medium", "remainingFraction": 1, "resetTime": "2026-10-24T23:30:55Z" },
        ] });
        let rows = pools(&v);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].label.as_str(), rows[1].label.as_str()), ("Claude · GPT", "Gemini"));
        assert!((rows[1].fraction - 0.25).abs() < 1e-6 && rows[0].fraction == 0.0);
    }

    #[test]
    fn odd_model_ids_never_panic() {
        let v = json!({ "buckets": [
            { "modelId": "-x", "remainingFraction": 0.5, "resetTime": "t" },
            { "modelId": "élan-1", "remainingFraction": 0.5, "resetTime": "t" },
        ] });
        let rows = pools(&v);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "-x · Élan");
    }

    /// Real quota of the signed-in agy: `cargo test -- --ignored agy_quota_live --nocapture`.
    #[tokio::test]
    #[ignore = "agy installé et connecté"]
    async fn agy_quota_live() {
        let bin = binary().expect("agy installé");
        let rows = usage(&crate::infra::http::client("").unwrap(), &bin).await.expect("quota");
        for w in &rows {
            println!("{} {:.1} % {:?}", w.label, w.fraction * 100.0, w.resets);
        }
    }

    /// Real run with the installed, signed-in agy: `cargo test -- --ignored agy_live --nocapture`.
    #[tokio::test]
    #[ignore = "agy installé et connecté"]
    async fn agy_live() {
        let bin = binary().expect("agy installé");
        println!("modèles : {:?}", models(&bin).await);
        let done = run(&bin, "gemini-3.8-flash", Effort::Low, &[json!({ "role": "user", "content": "Reply with the single word: ok" })], |_| {})
            .await
            .unwrap();
        println!("{:?} {:?}", done.content, done.usage);
        assert!(done.content.to_lowercase().contains("ok"));
    }
}
