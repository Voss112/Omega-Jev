# Abonnements Claude & Gemini — Plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Exécuter Omega avec un abonnement Claude (Pro/Max) et un abonnement Gemini (Code Assist) au lieu de clés API, avec choix du moteur *et* de l'effort depuis le composeur et affichage de l'utilisation.

**Architecture:** Deux couches séparées. (1) `infra/auth/` : OAuth PKCE par fournisseur, jetons scellés sur disque (DPAPI), rafraîchissement transparent. (2) `infra/llm/` : une façade `Llm` qui traduit le JSON de conversation canonique d'`app::context` vers trois protocoles filaires (OpenAI-compatible inchangé, Anthropic Messages, Gemini `v1internal`) et normalise les deltas de streaming. `domain::engine::Engine` passe à 4 variantes et gagne `Effort` réglable ; `Event` transporte l'état de connexion et l'utilisation d'abonnement jusqu'à l'UI egui.

**Tech Stack:** Rust 2024, reqwest+rustls, tokio, serde_json, base64 0.23, ring 0.17 (SHA-256 PKCE), eframe/egui 0.37.

**Spec:** demande utilisateur du 2026-10-23 ; constantes Gemini vérifiées dans `google-gemini/gemini-cli` (`packages/core/src/code_assist/oauth2.ts` et `server.ts`).

## Global Constraints

- Edition 2024, `cargo clippy --all-targets -- -D warnings` propre, pas d'`unwrap` hors tests, pas de `todo!()`.
- Aucune nouvelle dépendance sauf `ring = "0.17"` (déjà dans `Cargo.lock` en 0.17.14). Pas de `sha2`, pas de `urlencoding`, pas de crate OAuth.
- Aucun jeton, code OAuth ou secret dans un log, un message d'erreur, un nom de fichier ou l'UI. Les jetons ne sortent jamais de `infra/auth` et ne voyagent qu'en HTTPS.
- Jetons au repos : `secret::seal` (DPAPI) dans `%APPDATA%\Omega\credentials\<provider>.json`, une seule ligne chiffrée, permissions propriétaire.
- Les URL de fournisseur restent modifiables dans les paramètres ; toute URL non `https://` est refusée (sauf le `redirect_uri` loopback `http://127.0.0.1:<port>/oauth2callback` côté Google, imposé par Google).
- UI, erreurs et libellés en français, style télégraphique existant. Fichiers < 400 lignes, fonctions courtes.
- Vérification à chaque tâche : `cargo check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`.
- Les tests réseau réels sont `#[ignore]` et pilotés par variables d'environnement (`OMEGA_TEST_MODEL`).

## Review Focus

1. **Jeton expiré en milieu de conversation** — l'utilisateur s'attend à ce que le tour continue après un rafraîchissement silencieux, jamais à une erreur 401 affichée → test dans la tâche 2 (`needs_refresh` avec horloge fixe) et tâche 8 (`fresh` rafraîchit une seule fois).
2. **Appels d'outils en streaming coupés en plusieurs deltas JSON** — l'utilisateur s'attend à ce que l'outil s'exécute, pas à un `arguments` vide → fixtures dans les tâches 6 et 7.
3. **`arguments` vide ou JSON partiel** renvoyé par le modèle — l'utilisateur s'attend à `{}` et à ce que l'outil refuse proprement, pas à un panic → test tâche 6.
4. **Historique réparé contenant un `tool_result` sans `tool_use`** (conversation interrompue) — l'utilisateur s'attend à ce que le tour reparte → test tâche 6 (message orphelin converti en texte utilisateur).
5. **Réponse d'erreur HTTP non JSON ou 429** — l'utilisateur s'attend à un message lisible et à une reprise, pas à un écran vide → test tâche 6 (`api_error`) et tâche 8 (retry unique).

## Pré-requis d'exécution

Le dossier de travail n'est **pas** un dépôt git (`git rev-parse` échoue). La tâche 0 initialise le versionnement pour que chaque tâche puisse être commitée.

---

### Tâche 0 : Versionnement initial

**Files:**
- Create: `.gitignore`

**Interfaces:**
- Consumes: rien.
- Produces: dépôt git avec une référence de retour arrière par tâche.

- [ ] **Step 1: Créer `.gitignore`**

```gitignore
/target
```

- [ ] **Step 2: Initialiser et commiter**

```powershell
git init
git add -A
git commit -m "chore: baseline avant abonnements Claude/Gemini"
```

Expected: commit créé, `git status` propre (hors `/target`).

---

### Tâche 1 : `Engine` à 4 variantes + `Effort` réglable

**Files:**
- Modify: `src/domain/engine.rs` (réécriture complète, ~90 lignes)
- Modify: `src/domain/settings.rs:80-116`
- Modify: `src/infra/llm.rs:56-75` (mapping effort)
- Test: `src/domain/engine.rs` (module `tests`), `src/infra/llm.rs:275-287`

**Interfaces:**
- Consumes: rien.
- Produces:
  - `Engine::{Kimi, DeepSeek, Claude, Gemini}`, `Engine::ALL: [Engine; 4]`, `Engine::label(self) -> &'static str`, `Engine::key_var(self) -> &'static str`, `Engine::default_effort(self) -> Effort`, `Engine::wire(self) -> Wire` ; `pub enum Wire { OpenAi, Anthropic, Gemini }` défini dans `src/domain/engine.rs`.
  - `Effort::{Off, Low, Medium, High, Max}`, `Effort::ALL: [Effort; 5]`, `Effort::label(self) -> &'static str`, `Effort::openai(self) -> Option<&'static str>` (None pour Off), `Effort::claude_budget(self) -> Option<u32>` (`Off→None, Low→1024, Medium→4096, High→8192, Max→16384`), `Effort::gemini_budget(self) -> i32` (`Off→0, Low→1024, Medium→4096, High→8192, Max→-1`).
  - `Efforts { kimi, deepseek, claude, gemini }` + `Efforts::get(self, Engine) -> Effort`, `Efforts::set(&mut self, Engine, Effort)`.
  - Create `src/domain/auth.rs` avec seulement `pub enum AuthMode { ApiKey, Subscription }` (`#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]`, `#[serde(rename_all = "snake_case")]`, `Default = Subscription`), déclaré dans `src/domain/mod.rs`.
  - `Settings` gagne : `claude_key, claude_url, claude_model, claude_auth: AuthMode`, `gemini_key, gemini_url, gemini_model, gemini_auth: AuthMode`, `effort: Efforts`. `Settings::endpoint(Engine) -> (&str, &str, &str)` couvre les 4 moteurs.

- [ ] **Step 1: Écrire les tests qui échouent**

```rust
// src/domain/engine.rs
#[test]
fn defaults_and_labels_cover_four_engines() {
    assert_eq!(Engine::ALL.len(), 4);
    assert_eq!(Engine::default_effort(Engine::Kimi), Effort::Low);
    assert_eq!(Engine::default_effort(Engine::DeepSeek), Effort::Max);
    assert_eq!(Engine::wire(Engine::Claude), Wire::Anthropic);
    assert_eq!(Engine::wire(Engine::Gemini), Wire::Gemini);
    assert_eq!(Engine::wire(Engine::Kimi), Wire::OpenAi);
}

#[test]
fn effort_maps_to_each_provider() {
    assert_eq!(Effort::Off.openai(), None);
    assert_eq!((Effort::Low.claude_budget(), Effort::Max.claude_budget()), (Some(1024), Some(16384)));
    assert_eq!((Effort::Off.gemini_budget(), Effort::Max.gemini_budget()), (0, -1));
}
```

```rust
// src/infra/llm.rs (remplace `bodies_carry_each_engine_fixed_effort`)
#[test]
fn bodies_carry_the_selected_effort() {
    let (http, mut settings) = (crate::infra::http::client("").unwrap(), Settings::default());
    let msg = [json!({ "role": "user", "content": "x" })];
    settings.effort.set(Engine::Kimi, Effort::High);
    let kimi = Llm::new(http.clone(), &settings, Engine::Kimi).body(&msg, None);
    assert_eq!((kimi["model"].as_str(), kimi["reasoning_effort"].as_str()), (Some("kimi-k3.1-pro"), Some("high")));
    settings.effort.set(Engine::Kimi, Effort::Off);
    assert!(Llm::new(http, &settings, Engine::Kimi).body(&msg, None).get("reasoning_effort").is_none());
}
```

- [ ] **Step 2: Lancer les tests, vérifier l'échec**

Run: `cargo test --lib domain::engine infra::llm::tests::bodies`
Expected: FAIL (`no variant or associated item named Claude`, `Effort` inconnu).

- [ ] **Step 3: Implémenter `Engine`, `Wire`, `Effort`, `Efforts`**

`src/domain/engine.rs` déclare `Wire` et `Effort` avec `#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]` et `#[serde(rename_all = "snake_case")]` ; `Efforts` avec `#[derive(Clone, Copy, Debug, Serialize, Deserialize)] #[serde(default)]` et un `Default` manuel (Kimi `Low`, DeepSeek `Max`, Claude `Medium`, Gemini `Medium`). Libellés : `"Claude"`, `"Gemini"`, `"Kimi K3.1 Pro"`, `"DeepSeek V4.1 Flash"` ; `key_var` : `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `MOONSHOT_API_KEY`, `DEEPSEEK_API_KEY`.

- [ ] **Step 4: Étendre `Settings` et son `Default`**

Ajouter les 8 champs fournisseur, `effort`, et dans `Default` : `claude_url: "https://api.anthropic.com".into()`, `claude_model: "claude-sonnet-4-5".into()`, `claude_auth: AuthMode::Subscription`, `gemini_url: "https://generativelanguage.googleapis.com/v1beta/openai".into()`, `gemini_model: "gemini-2.5-pro".into()`, `gemini_auth: AuthMode::Subscription`.

- [ ] **Step 5: Mapper l'effort dans `Llm::body`**

```rust
if let Some(e) = self.effort.openai() {
    body["reasoning_effort"] = json!(e);
}
match self.engine {
    Engine::Kimi => body["max_completion_tokens"] = json!(MAX_OUTPUT),
    Engine::DeepSeek => {
        body["max_tokens"] = json!(MAX_OUTPUT);
        body["thinking"] = json!({ "type": "enabled" });
    }
    Engine::Claude | Engine::Gemini => {}
}
```

`Llm` gagne `pub effort: Effort`, alimenté par `settings.effort.get(engine)` dans `Llm::new`.

- [ ] **Step 6: Rejouer les tests**

Run: `cargo test --lib domain::engine infra::llm::tests`
Expected: PASS. `cargo check` peut encore signaler les `match` non exhaustifs de `src/ui/*` et `src/app/health.rs` : ils sont traités tâche 9 et tâche 8 ; en attendant, élargir chaque `match engine` avec `Engine::Claude | Engine::Gemini => {}` **sans changer le comportement**.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat(domain): quatre moteurs et effort reglable"
```

---

### Tâche 2 : Socle d'authentification (types + jetons scellés)

**Files:**
- Create: `src/domain/auth.rs`
- Create: `src/infra/auth/mod.rs`
- Modify: `src/domain/mod.rs`, `src/infra/mod.rs`, `src/infra/paths.rs:35-40`

**Interfaces:**
- Consumes: `infra::secret::{seal, open}`, `infra::paths::root`.
- Produces:
  - `domain::auth::{AuthMode, Provider, Cred, Window}` avec `AuthMode::{ApiKey, Subscription}` (créé tâche 1), `Provider::{Claude, Gemini}`, `Provider::id(self) -> &'static str` (`"claude"`, `"gemini"`), `Provider::label(self) -> &'static str`, `Cred { access: String, refresh: String, expires_at: i64, account: String, project: String }`, `Window { label: String, fraction: f32, resets: Option<String> }`.
  - `infra::auth::{Auth, load, save_cred, refresh}` : `Auth::load(provider) -> Option<Arc<Auth>>`, `Auth::save(&self, cred: Cred) -> Result<()>`, `Auth::access(&self) -> String`, `Auth::account(&self) -> String`, `Auth::needs_refresh(&self, now: i64) -> bool`, `Auth::replace(&self, cred: Cred)`.
  - `paths::creds() -> PathBuf`.

- [ ] **Step 1: Écrire le test qui échoue**

```rust
// src/infra/auth/mod.rs
#[cfg(test)]
mod tests {
    use crate::domain::auth::{Cred, Provider};
    #[test]
    fn stale_tokens_are_refreshed_before_expiry() {
        let cred = Cred { access: "a".into(), refresh: "r".into(), expires_at: 1_000, account: String::new(), project: String::new() };
        assert!(super::stale(&cred, 900));      // 100 s restantes
        assert!(!super::stale(&cred, 800));     // 200 s restantes
        assert!(super::stale(&Cred { expires_at: 0, ..cred }, 1)); // inconnu = à rafraîchir
    }
    #[test]
    fn cred_round_trips_through_the_sealed_file() {
        let dir = std::env::temp_dir().join(format!("omega-creds-{}", crate::domain::auth::Provider::Claude.id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cred = Cred { access: "at-123".into(), refresh: "rt-456".into(), expires_at: 42, account: "me@x".into(), project: String::new() };
        super::write_to(&dir, Provider::Claude, &cred).unwrap();
        let raw = std::fs::read_to_string(dir.join("claude.json")).unwrap();
        assert!(!raw.contains("at-123") && !raw.contains("rt-456"), "jeton en clair sur disque");
        assert_eq!(super::read_from(&dir, Provider::Claude).unwrap().refresh, "rt-456");
    }
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test --lib infra::auth`
Expected: FAIL (`unresolved import crate::domain::auth`).

- [ ] **Step 3: Écrire `src/domain/auth.rs`**

Ajouter à `src/domain/auth.rs` les types `Provider`, `Cred`, `Window` (ci-dessus), tous `#[derive(Clone, Debug, Default, Serialize, Deserialize)]` sauf `Provider` (`Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize`, `#[serde(rename_all = "snake_case")]`).

- [ ] **Step 4: Écrire `src/infra/auth/mod.rs`**

```rust
pub fn stale(cred: &Cred, now: i64) -> bool { cred.expires_at == 0 || cred.expires_at - now < 120 }
pub fn write_to(dir: &Path, p: Provider, c: &Cred) -> Result<()>;
pub fn read_from(dir: &Path, p: Provider) -> Option<Cred>;
pub async fn refresh(http: &Client, p: Provider, c: &Cred) -> Result<Cred>;
```

`write_to` sérialise `Cred` en JSON puis `secret::seal` sur la chaîne entière et écrit via un `tmp` + rename (copier le motif de `store::write_private`). `read_from` fait l'inverse et renvoie `None` si le fichier manque ou ne se déchiffre pas. `Auth` garde un `Mutex<Cred>` + le `dir` ; `Auth::load` renvoie `None` s'il n'y a pas de fichier valide. `refresh` délègue à `claude::refresh` / `gemini::refresh` (tâches 4 et 5) : laisser `bail!("fournisseur inconnu")` pour l'instant.

- [ ] **Step 5: Ajouter `paths::creds()`**

```rust
pub fn creds() -> PathBuf {
    root().join("credentials")
}
```
et l'ajouter à la liste de `paths::ensure()`.

- [ ] **Step 6: Rejouer les tests**

Run: `cargo test --lib infra::auth`
Expected: PASS (2 tests).

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat(auth): creds scellees et detection d'expiration"
```

---

### Tâche 3 : PKCE + capture loopback

**Files:**
- Create: `src/infra/auth/pkce.rs`
- Modify: `src/infra/auth/mod.rs` (`pub mod pkce;`), `Cargo.toml` (ajout `ring = "0.17"`)

**Interfaces:**
- Consumes: `ring::digest`, `base64::engine::general_purpose::URL_SAFE_NO_PAD`, `tokio::net::TcpListener`.
- Produces:
  - `Pkce { verifier: String, challenge: String, state: String }`, `Pkce::new() -> Pkce`.
  - `challenge(verifier: &str) -> String` (SHA-256 → base64url sans padding).
  - `encode(s: &str) -> String` (percent-encoding RFC 3986 unreserved).
  - `pub async fn await_code(port: u16, state: &str, timeout: Duration) -> Result<String>` — écoute `http://127.0.0.1:{port}/oauth2callback`, vérifie `state`, répond un HTML minimal « Connexion réussie, revenez à Omega ».

- [ ] **Step 1: Tests qui échouent (vecteur RFC 7636 + loopback réel)**

```rust
#[test]
fn rfc7636_vector() {
    assert_eq!(
        super::challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    assert_eq!(super::encode("a b/c:d@e"), "a%20b%2Fc%3Ad%40e");
    let p = super::Pkce::new();
    assert_eq!(p.verifier.len(), 43);
    assert_eq!(p.challenge, super::challenge(&p.verifier));
}

#[tokio::test]
async fn loopback_captures_the_code_once() {
    let mut pkce = super::Pkce::new();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let expected_state = std::mem::take(&mut pkce.state);
    let server = tokio::spawn(super::await_code(port, &expected_state, std::time::Duration::from_secs(5)));
    let _ = tokio::net::TcpStream::connect(("127.0.0.1", port)).await
        .unwrap()
        .writable()
        .await;
    // envoie la requête brute
    use tokio::io::AsyncWriteExt;
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(format!("GET /oauth2callback?state={expected_state}&code=abc123 HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).await.unwrap();
    assert_eq!(server.await.unwrap().unwrap(), "abc123");
}

#[tokio::test]
async fn loopback_rejects_a_foreign_state() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let server = tokio::spawn(super::await_code(port, "good", std::time::Duration::from_secs(5)));
    use tokio::io::AsyncWriteExt;
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(b"GET /oauth2callback?state=bad&code=abc123 HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
    assert!(server.await.unwrap().is_err());
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test --lib infra::auth::pkce`
Expected: FAIL (`module pkce` absent, `ring` non résolu).

- [ ] **Step 3: Implémenter `pkce.rs`**

Verifier : 43 caractères tirés de 32 octets aléatoires de `ring::rand::SystemRandom` encodés base64url sans padding. `state` : 32 octets hex. `await_code` : `TcpListener::bind(("127.0.0.1", port))`, `tokio::time::timeout`, lit jusqu'à `\r\n\r\n` (max 8 KiB), parse la ligne de requête, refuse tout `state` différent, écrit `HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: N\r\n\r\n<html>…</html>`.

- [ ] **Step 4: Rejouer**

Run: `cargo test --lib infra::auth::pkce`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(auth): pkce S256 et capture loopback"
```

---

### Tâche 4 : Fournisseur Claude (OAuth abonnement)

**Files:**
- Create: `src/infra/auth/claude.rs`
- Modify: `src/infra/auth/mod.rs` (`pub mod claude;` + dispatch de `refresh`)

**Interfaces:**
- Consumes: `super::pkce`, `domain::auth::{Cred, Provider, Window}`.
- Produces:
  - constantes publiques : `AUTHORIZE: &str = "https://claude.ai/oauth/authorize"`, `TOKEN: &str = "https://console.anthropic.com/v1/oauth/token"`, `CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"`, `REDIRECT: &str = "https://console.anthropic.com/oauth/code/callback"`, `SCOPE: &str = "org:create_api_key user:profile user:inference"`, `BETA: &str = "oauth-2025-04-20"`, `SYSTEM_PREFIX: &str = "You are Claude Code, Anthropic's official CLI for Claude."`, `API: &str = "https://api.anthropic.com"`, `USAGE: &str = "https://api.anthropic.com/api/oauth/usage"`.
  - `authorize_url(pkce: &Pkce) -> String`
  - `pub async fn exchange(http: &Client, code: &str, pkce: &Pkce) -> Result<Cred>`
  - `pub async fn refresh(http: &Client, cred: &Cred) -> Result<Cred>`
  - `pub fn import_cli(dir: &Path) -> Option<Cred>` — lit `<profil>\.claude\.credentials.json`, clé `claudeAiOauth.{accessToken, refreshToken, expiresAt, subscriptionType}` ; `expiresAt` est en **millisecondes** → conversion en secondes.
  - `pub async fn usage(http: &Client, access: &str) -> Option<Vec<Window>>` — appelle `windows` sur le JSON de `USAGE`, et renvoie `None` sur toute erreur (`five_hour`, `seven_day`, `seven_day_opus` → `fraction = utilization / 100`, `resets` = `resets_at`).
  - `pub fn windows(v: &Value) -> Vec<Window>` — parsing pur, testable hors réseau.

- [ ] **Step 1: Écrire les tests qui échouent**

```rust
#[test]
fn authorize_url_carries_pkce_and_scope() {
    let p = super::super::pkce::Pkce::new();
    let url = super::authorize_url(&p);
    assert!(url.starts_with("https://claude.ai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e"));
    assert!(url.contains("response_type=code"));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(url.contains(&format!("code_challenge={}", p.challenge)));
    assert!(url.contains(&format!("state={}", p.state)));
    assert!(url.contains("redirect_uri=https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fcode%2Fcallback"));
    assert!(url.contains("scope=org%3Acreate_api_key%20user%3Aprofile%20user%3Ainference"));
}

#[test]
fn cli_credentials_are_imported_in_seconds() {
    let dir = std::env::temp_dir().join("omega-claude-cli");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"at","refreshToken":"rt","expiresAt":1700000000000,"subscriptionType":"max"}}"#).unwrap();
    let c = super::import_cli(&dir).unwrap();
    assert_eq!((c.access.as_str(), c.refresh.as_str(), c.expires_at, c.account.as_str()), ("at", "rt", 1_700_000_000, "max"));
}

#[test]
fn usage_windows_are_read_leniently() {
    let v = serde_json::json!({ "five_hour": { "utilization": 42.5, "resets_at": "2026-10-24T02:00:00Z" }, "weird": 3 });
    let w = super::windows(&v);
    assert_eq!(w.len(), 1);
    assert!((w[0].fraction - 0.425).abs() < 1e-6 && w[0].resets.as_deref() == Some("2026-10-24T02:00:00Z"));
    assert!(super::windows(&serde_json::json!({})).is_empty());
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test --lib infra::auth::claude`
Expected: FAIL (`module claude` absent).

- [ ] **Step 3: Implémenter `claude.rs`**

`authorize_url` construit la query dans cet ordre exact : `code=true`, `client_id`, `response_type=code`, `redirect_uri`, `scope`, `code_challenge`, `code_challenge_method=S256`, `state`, chaque valeur via `pkce::encode`. `exchange` : POST JSON `{grant_type:"authorization_code", code, state, client_id, redirect_uri, code_verifier}` sur `TOKEN`, réponse `{access_token, refresh_token, expires_in}` → `Cred { expires_at: now + expires_in, account: "claude", .. }`. `refresh` : POST JSON `{grant_type:"refresh_token", refresh_token, client_id}` ; conserve `refresh` si la réponse n'en renvoie pas. `usage` n'est appelée que si le mode est Subscription et renvoie `None` sur toute erreur (403, 404, JSON inattendu).

- [ ] **Step 4: Brancher `refresh` dans `infra/auth/mod.rs`**

```rust
match p {
    Provider::Claude => claude::refresh(http, c).await,
    Provider::Gemini => gemini::refresh(http, c).await, // livré tâche 5
}
```

- [ ] **Step 5: Rejouer**

Run: `cargo test --lib infra::auth`
Expected: PASS.

- [ ] **Step 6: Vérification manuelle (hors CI)**

Lancer l'URL produite par `authorize_url` dans un navigateur, valider avec le compte Claude, coller le code affiché : `exchange` doit produire un fichier `%APPDATA%\Omega\credentials\claude.json`. Si le serveur répond 400/401, utiliser le bouton d'import (test 2 de la tâche) et consigner l'erreur exacte : c'est la seule sortie qui autorise à modifier `CLIENT_ID`/`REDIRECT`/`SCOPE`.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat(auth): oauth claude abonnement"
```

---

### Tâche 5 : Fournisseur Gemini (Code Assist)

**Files:**
- Create: `src/infra/auth/gemini.rs`
- Modify: `src/infra/auth/mod.rs` (`pub mod gemini;`)

**Interfaces:**
- Consumes: `super::pkce`, `domain::auth::Cred`.
- Produces:
  - `AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth"`, `TOKEN: &str = "https://oauth2.googleapis.com/token"`, `CLIENT_ID`, `CLIENT_SECRET = "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"`, `SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile"`, `CODE_ASSIST: &str = "https://cloudcode-pa.googleapis.com/v1internal"`.
  - `authorize_url(pkce: &Pkce, redirect: &str) -> String` (`access_type=offline`, `prompt=consent`, `response_type=code`, `include_granted_scopes=true`).
  - `pub async fn exchange(http: &Client, code: &str, pkce: &Pkce, redirect: &str) -> Result<Cred>`
  - `pub async fn refresh(http: &Client, cred: &Cred) -> Result<Cred>`
  - `pub async fn tier(http: &Client, access: &str, project: &str) -> Result<(String, String)>` — `(tier_id, project)` via `loadCodeAssist`, puis `onboardUser` + polling de l'opération si aucun projet.
  - `pub fn tier_of(v: &Value) -> Option<(String, String)>` — lecture pure de `{currentTier.id, cloudaicompanionProject}`.
  - `pub fn envelope(project: &str, model: &str, request: Value) -> Value`

- [ ] **Step 1: Tests qui échouent**

```rust
#[test]
fn authorize_url_matches_the_desktop_client_flow() {
    let p = super::super::pkce::Pkce::new();
    let url = super::authorize_url(&p, "http://127.0.0.1:41234/oauth2callback");
    assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
    for needle in [
        "client_id=681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com",
        "response_type=code",
        "access_type=offline",
        "prompt=consent",
        "code_challenge_method=S256",
        "redirect_uri=http%3A%2F%2F127.0.0.1%3A41234%2Foauth2callback",
    ] {
        assert!(url.contains(needle), "manque {needle}");
    }
    assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcloud-platform"));
}

#[test]
fn request_envelope_wraps_model_and_project() {
    let body = super::envelope("proj-1", "gemini-2.5-pro", serde_json::json!({ "contents": [] }));
    assert_eq!(body["model"], "gemini-2.5-pro");
    assert_eq!(body["project"], "proj-1");
    assert!(body.get("request").is_some());
}

#[test]
fn tier_is_read_from_load_code_assist() {
    let v = serde_json::json!({ "currentTier": { "id": "standard" }, "cloudaicompanionProject": "proj-9" });
    assert_eq!(super::tier_of(&v), Some(("standard".to_owned(), "proj-9".to_owned())));
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test --lib infra::auth::gemini`
Expected: FAIL.

- [ ] **Step 3: Implémenter `gemini.rs`**

`exchange` : POST formulaire `application/x-www-form-urlencoded` sur `TOKEN` avec `client_id`, `client_secret`, `code`, `code_verifier`, `grant_type=authorization_code`, `redirect_uri`. Réponse `{access_token, refresh_token, expires_in}` → `Cred { expires_at: now+expires_in, account: <email via userinfo>, project: String::new() }`. `refresh` : même endpoint, `grant_type=refresh_token`. `tier` : POST `{CODE_ASSIST}:loadCodeAssist` body `{"cloudaicompanionProject": project, "metadata": {"pluginType": "GEMINI"}}` ; si `cloudaicompanionProject` absent, POST `{CODE_ASSIST}:onboardUser` body `{"tierId": tier, "metadata": {"pluginType": "GEMINI"}}`, puis `GET {CODE_ASSIST}/<operation.name>` toutes les 2 s pendant 30 s maximum jusqu'à `done == true`, et lire `response.cloudaicompanionProject`. Le nom d'opération est validé (`[A-Za-z0-9/_-]+`) avant concaténation.

- [ ] **Step 4: Rejouer**

Run: `cargo test --lib infra::auth::gemini`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(auth): oauth gemini code assist et resolution du projet"
```

---

### Tâche 6 : Éclatement de `Llm` + fil Anthropic Messages

**Files:**
- Create: `src/infra/llm/mod.rs`, `src/infra/llm/openai.rs`, `src/infra/llm/anthropic.rs`
- Delete: `src/infra/llm.rs` (déplacé)
- Modify: `src/infra/mod.rs` (inchangé : `pub mod llm;` pointe désormais sur le dossier)

**Interfaces:**
- Consumes: `domain::engine::{Engine, Effort, Wire}`, `infra::auth::Auth`.
- Produces: `llm::{Llm, Delta, Completion}` ; `Llm::{new, configured, body, models, ping, stream}` ; nouveaux modules `openai`, `anthropic`, `gemini` avec, chacun, `pub fn body(...) -> Value` et `pub(crate) fn apply(out: &mut Completion, data: &str, on: &mut impl FnMut(Delta<'_>)) -> Result<()>`.
  - `anthropic::body(messages: &[Value], tools: Option<Value>, model: &str, effort: Effort, oauth: bool) -> Value`
  - `anthropic::error_text(body: &str) -> String` (reprend `api_error` d'`openai.rs`).

- [ ] **Step 1: Déplacer sans rien changer**

```powershell
New-Item -ItemType Directory src\infra\llm
Move-Item src\infra\llm.rs src\infra\llm\openai.rs
```
Créer `mod.rs` avec `mod anthropic; mod gemini; mod openai;` et les ré-exports. Run: `cargo test --lib` → PASS (comportement identique).

- [ ] **Step 2: Tests Anthropic qui échouent**

```rust
// src/infra/llm/anthropic.rs
#[test]
fn messages_are_converted_with_tools() {
    let msgs = vec![
        json!({ "role": "system", "content": "SYS" }),
        json!({ "role": "user", "content": "hi" }),
        json!({ "role": "assistant", "content": "", "tool_calls": [
            { "id": "t1", "type": "function", "function": { "name": "read_file", "arguments": "{\"path\":\"a.rs\"}" } }
        ]}),
        json!({ "role": "tool", "tool_call_id": "t1", "content": "1  code" }),
    ];
    let b = super::body(&msgs, None, "claude-sonnet-4-5", Effort::Medium, true);
    assert_eq!(b["system"][0]["text"], super::SYSTEM_PREFIX);
    assert_eq!(b["system"][1]["text"], "SYS");
    assert_eq!(b["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(b["messages"][1]["content"][0]["input"]["path"], "a.rs");
    assert_eq!(b["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(b["thinking"]["budget_tokens"], 4096);
    assert_eq!(b["stream"], true);
}

#[test]
fn empty_tool_arguments_become_an_empty_object() {
    let msgs = vec![json!({ "role": "assistant", "content": "", "tool_calls": [
        { "id": "t1", "type": "function", "function": { "name": "shell", "arguments": "" } } ]})];
    let b = super::body(&msgs, None, "m", Effort::Off, false);
    assert_eq!(b["messages"][0]["content"][0]["input"], json!({}));
    assert!(b.get("thinking").is_none());
    assert_eq!(b["system"][0]["text"], "SYS"); // sans OAuth, un seul bloc système
}

#[test]
fn orphan_tool_results_become_user_text() {
    let msgs = vec![json!({ "role": "tool", "tool_call_id": "gone", "content": "ERROR" })];
    let b = super::body(&msgs, None, "m", None, false);
    assert_eq!(b["messages"][0]["role"], "user");
}

#[test]
fn stream_events_accumulate_text_thinking_and_input_json() {
    let mut out = crate::infra::llm::Completion::default();
    for e in [
        r#"{"type":"message_start","message":{"usage":{"input_tokens":10}}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"why"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
        r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"grep"}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"pat"}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"tern\":\"x\"}"}}"#,
        r#"{"type":"message_delta","usage":{"output_tokens":5}}"#,
    ] {
        super::apply(&mut out, e, &mut |_| {}).unwrap();
    }
    assert_eq!((out.content.as_str(), out.reasoning.as_str()), ("ok", "why"));
    assert_eq!(out.tool_calls[0].arguments, r#"{"pattern":"x"}"#);
    assert_eq!((out.usage.prompt, out.usage.completion), (10, 5));
}

#[test]
fn api_error_reads_the_message_field() {
    assert_eq!(super::error_text(r#"{"error":{"message":"bad key"}}"#), "bad key");
    assert_eq!(super::error_text("<html>oops</html>"), "<html>oops</html>");
}
```

- [ ] **Step 3: Lancer, vérifier l'échec**

Run: `cargo test --lib infra::llm::anthropic`
Expected: FAIL.

- [ ] **Step 4: Implémenter `anthropic.rs`**

`body(messages, tools, model, effort, oauth) -> Value` : extrait les messages `system` dans `system` (tableau de blocs `{type:"text", text}` ; en mode OAuth le bloc `SYSTEM_PREFIX` est inséré **en premier**), convertit `assistant.tool_calls` en blocs `tool_use` (arguments parsés, `{}` si vide ou invalide), convertit `tool` en blocs `tool_result` groupés dans un message `user` (un `tool_result` orphelin devient un message `user` texte). Outils : `[{name, description, input_schema}]` extraits de `function.{name,description,parameters}`. `max_tokens` = `budget + 8192` borné à 32 000, ou 8192 sans réflexion. `thinking` omis si non OAuth ou si `Effort::Off`.

- [ ] **Step 5: Rejouer**

Run: `cargo test --lib infra::llm::anthropic`
Expected: PASS (5 tests).

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(llm): fil anthropic messages"
```

---

### Tâche 7 : Fil Gemini Code Assist (`v1internal`)

**Files:**
- Create: `src/infra/llm/gemini.rs`
- Modify: `src/infra/llm/mod.rs`

**Interfaces:**
- Consumes: `domain::engine::Effort`, `auth::gemini::envelope`.
- Produces: `gemini::body(messages: &[Value], tools: Option<Value>, model: &str, project: &str, effort: Effort) -> Value`, `gemini::apply(out: &mut Completion, data: &str, on: &mut impl FnMut(Delta<'_>)) -> Result<()>`, `gemini::unwrap(v: &Value) -> &Value` (accepte la réponse enveloppée `{"response":{…}}` ou nue).

- [ ] **Step 1: Tests qui échouent**

```rust
#[test]
fn history_maps_to_contents_and_function_calls() {
    let msgs = vec![
        json!({ "role": "system", "content": "SYS" }),
        json!({ "role": "user", "content": "hi" }),
        json!({ "role": "assistant", "content": "", "tool_calls": [
            { "id": "c1", "type": "function", "function": { "name": "grep", "arguments": "{\"pattern\":\"x\"}" } } ]}),
        json!({ "role": "tool", "tool_call_id": "c1", "content": "1 hit" }),
    ];
    let b = super::body(&msgs, None, "gemini-2.5-pro", "p1", Effort::High);
    assert_eq!(b["model"], "gemini-2.5-pro");
    assert_eq!(b["project"], "p1");
    assert_eq!(b["request"]["systemInstruction"]["parts"][0]["text"], "SYS");
    assert_eq!(b["request"]["contents"][1]["role"], "model");
    assert_eq!(b["request"]["contents"][1]["parts"][0]["functionCall"]["name"], "grep");
    assert_eq!(b["request"]["contents"][2]["parts"][0]["functionResponse"]["name"], "grep");
    assert_eq!(b["request"]["generationConfig"]["thinkingConfig"]["thinkingBudget"], 8192);
}

#[test]
fn tool_response_without_name_is_skipped() {
    let msgs = vec![json!({ "role": "tool", "tool_call_id": "gone", "content": "x" })];
    let b = super::body(&msgs, None, "m", "p", Effort::Off);
    assert_eq!(b["request"]["contents"].as_array().unwrap().len(), 0);
}

#[test]
fn streaming_parts_and_usage_are_read() {
    let mut out = crate::infra::llm::Completion::default();
    for e in [
        r#"{"response":{"candidates":[{"content":{"parts":[{"text":"ok"}]}}]}}"#,
        r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"grep","args":{"pattern":"x"}}}]}}]}"#,
        r#"{"response":{"candidates":[{"content":{"parts":[]}}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":4,"cachedContentTokenCount":9}}}"#,
    ] {
        super::apply(&mut out, e, &mut |_| {}).unwrap();
    }
    assert_eq!(out.content, "ok");
    assert_eq!(out.tool_calls[0].name, "grep");
    assert_eq!(out.tool_calls[0].arguments, r#"{"pattern":"x"}"#);
    assert_eq!((out.usage.prompt, out.usage.completion, out.usage.cached), (12, 4, 9));
    assert!(out.tool_calls[0].id.starts_with("gemini-"));
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test --lib infra::llm::gemini`
Expected: FAIL.

- [ ] **Step 3: Implémenter `gemini.rs`**

Le passage `tool` → `functionResponse` exige le **nom** de la fonction : construire d'abord une table `tool_call_id → name` en parcourant l'historique. Chaque `functionCall` reçoit un id synthétique déterministe `gemini-{index}` (l'API n'en fournit pas) pour que la réponse correspondante soit appairée. `generationConfig` : `maxOutputTokens: 65536`, `thinkingConfig.thinkingBudget` = `effort.gemini_budget()` ; outils : `[{ "functionDeclarations": [{name, description, parameters}] }]`.

- [ ] **Step 4: Rejouer**

Run: `cargo test --lib infra::llm::gemini`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(llm): fil gemini code assist"
```

---

### Tâche 8 : Façade `Llm` — modes d'auth, rafraîchissement, santé

**Files:**
- Modify: `src/infra/llm/mod.rs` (façade), `src/infra/llm/openai.rs` (extraction de `send`)
- Modify: `src/app/mod.rs:41-57`, `src/app/health.rs:30-99`

**Interfaces:**
- Consumes: `auth::{Auth, load}`, `anthropic`, `gemini`.
- Produces:
  - `Llm { http, engine, base, model, key, auth: Option<Arc<Auth>>, project: String, effort: Effort }`
  - `Llm::new(http: Client, settings: &Settings, engine: Engine) -> Self` : si `settings.<provider>_auth == Subscription` et qu'un `Auth` existe, `auth = Some(..)`, `project = auth.account()` ne sert pas — `project` vient de `Cred.project`.
  - `Llm::configured(&self) -> bool` : mode clé → clé non vide et base HTTPS ; mode abonnement → `auth.is_some()`.
  - `Llm::authorization(&self) -> Result<String>` : `fresh()` du jeton ou la clé statique.

- [ ] **Step 1: Tests qui échouent**

```rust
#[test]
fn subscription_mode_does_not_need_an_api_key() {
    let (http, mut s) = (crate::infra::http::client("").unwrap(), Settings::default());
    s.kimi_key = "sk-x".into();
    let llm = Llm::new(http.clone(), &s, Engine::Kimi);
    assert!(llm.configured());
    // Claude en abonnement sans fichier de jetons : non configuré, sans erreur
    assert!(!Llm::new(http, &s, Engine::Claude).configured());
}

#[test]
fn api_key_mode_selects_the_right_wire_and_error_retry() {
    let s = Settings { engine: Engine::Claude, claude_auth: AuthMode::ApiKey, claude_key: "k".into(), ..Settings::default() };
    let llm = Llm::new(crate::infra::http::client("").unwrap(), &s, Engine::Claude);
    assert!(llm.configured());
    assert!(matches!(super::retryable(429, 1), true));
    assert!(!super::retryable(401, 1));
    assert!(super::retryable(500, 3) == false, "plafond de tentatives");
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test --lib infra::llm::tests`
Expected: FAIL.

- [ ] **Step 3: Implémenter la façade**

`body` : dispatch sur `engine.wire()` (`openai::body` / `anthropic::body` / `gemini::body`) en passant `model`, `effort`, `oauth = auth.is_some()`, `project`. `stream` : un seul `send()` par fil, puis parsing ligne à ligne ; les lignes `data:` sont passées au `apply` du fil ; `[DONE]` termine (OpenAI seul). `send` : `authorization()` avant chaque tentative, en-têtes par fil — OpenAI `Authorization: Bearer`, Anthropic clé `x-api-key` + `anthropic-version: 2023-06-01`, Anthropic abonnement `Authorization: Bearer` + `anthropic-beta: oauth-2025-04-20` + `anthropic-version`, Gemini `Authorization: Bearer`. URL : `{base}/chat/completions`, `{base}/v1/messages`, `{base}:streamGenerateContent?alt=sse` (base Gemini = `https://cloudcode-pa.googleapis.com/v1internal`). Erreur 429/5xx : 3 tentatives maximum, `retryable(status, attempt)` ; 401 avec abonnement : un rafraîchissement forcé puis une seule reprise.

- [ ] **Step 4: Brancher `Env` et la santé**

`Env::new` construit `Llm::new(..., settings.engine)` comme avant (aucun changement de signature). `health::check` ajoute `engine(env, Engine::Claude)` et `engine(env, Engine::Gemini)` dans le `tokio::join!` et deux entrées « Moteurs IA ». `health::engine` affiche `abonnement`/`clé API` à la place du couple effort fixe.

- [ ] **Step 5: Rejouer**

Run: `cargo test --lib && cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(llm): facade multi-fournisseurs et rafraichissement des jetons"
```

---

### Tâche 9 : Composeur — moteur, effort, utilisation

**Files:**
- Modify: `src/ui/composer.rs:104-124`, `src/ui/panel.rs:110-139`, `src/ui/sidebar.rs:26`, `src/ui/chat.rs:305`

**Interfaces:**
- Consumes: `Settings::{engine, effort}`, `Efforts::{get,set}`, `Auth::account`, `domain::auth::Window`.
- Produces: `App` gagne `plan_usage: Option<(Provider, Vec<Window>, Instant)>` ; `Event::PlanUsage { provider: Provider, windows: Vec<Window> }` ; `composer::engine_label(Engine, AuthMode) -> String`, `composer::effort_label(Effort) -> String`.

- [ ] **Step 1: Test qui échoue (rendu headless egui)**

```rust
// src/ui/composer.rs
#[test]
fn labels_show_engine_mode_and_effort() {
    assert_eq!(super::engine_label(Engine::Kimi, AuthMode::ApiKey), "⚡ Kimi K3.1 Pro · clé API");
    assert_eq!(super::engine_label(Engine::Claude, AuthMode::Subscription), "⚡ Claude · abonnement");
    assert_eq!(super::effort_label(Effort::Off), "effort Off (sans réflexion)");
    assert_eq!(super::effort_label(Effort::Max), "effort Max");
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test --lib ui::composer`
Expected: FAIL.

- [ ] **Step 3: Implémenter**

`engine_picker` : premier `ComboBox` sur `Engine::ALL` avec `engine_label(engine, mode)` où `mode` est `settings.claude_auth`/`gemini_auth` pour ces deux moteurs, `ApiKey` sinon ; second `ComboBox` sur `Effort::ALL` avec `effort_label(e)` pour `app.settings.engine` uniquement ; toute modification passe par `apply_settings`. `panel::economy` ajoute sous les compteurs une ligne « Abonnement » : `Provider::label` + `account` + une barre par `Window` (`fraction`, libellé, `resets`), ou « non connecté » ; rien si l'engine actif est Kimi/DeepSeek. `sidebar`/`chat` remplacent `app.settings.engine.label()` par `format!("{} · effort {}", …)`.

- [ ] **Step 4: Rejouer**

Run: `cargo test --lib ui:: && cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(ui): switch moteur, effort et utilisation d'abonnement"
```

---

### Tâche 10 : Connexion Claude/Gemini dans les paramètres

**Files:**
- Create: `src/app/login.rs` (use case), `src/ui/login.rs` (modale)
- Modify: `src/app/mod.rs` (`Event::{Connected, PlanUsage}`, `pub mod login;`), `src/ui/mod.rs` (`mod login;`, champ `login`), `src/ui/settings.rs:67-140`, `src/ui/theme.rs` si besoin

**Interfaces:**
- Consumes: `auth::{claude, gemini, pkce, Auth}`, `Event`, `open`.
- Produces:
  - `app::login::{claude_start, gemini_start, finish_paste, import_cli}` : `claude_start() -> (String, ui::login::Login)` (URL d'autorisation + état de modale, aucun réseau), `gemini_start() -> (String, ui::login::Login)` (réserve le port loopback, `listening = true`), `finish_paste(http: &Client, login: &Login, code: &str) -> Result<Cred>`, `import_cli() -> Result<Cred>`.
  - `Event::Connected { provider: Provider, result: Result<Cred, String> }`
  - `ui::login::Login { provider, url, one_time: Option<String>, paste: String, status: String, listening: bool }` + `ui::login::show(app: &mut App, ctx: &egui::Context)`.

- [ ] **Step 1: Test qui échoue (échange et import sans réseau)**

```rust
// src/app/login.rs
#[test]
fn claude_url_is_ready_to_open() {
    let (url, login) = super::claude_start();
    assert!(url.starts_with("https://claude.ai/oauth/authorize?"));
    assert!(login.listening == false);
    assert_eq!(login.provider, crate::domain::auth::Provider::Claude);
}

#[test]
fn gemini_login_listens_on_a_free_loopback_port() {
    let (url, login) = super::gemini_start();
    assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A"));
    assert!(login.listening);
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test --lib app::login`
Expected: FAIL.

- [ ] **Step 3: Implémenter les cas d'usage**

`claude_start` : `Pkce::new()` → `authorize_url` → `Login { provider: Claude, url, one_time: Some(state), listening: false }` ; `finish_paste` échange le code collé, écrit le jeton (`Auth::save`) et émet `Event::Connected`. `gemini_start` : réserve un port libre (`TcpListener::bind("127.0.0.1:0")` puis `local_addr().port()`, listener refermé), construit `redirect_uri`, lance la tâche `pkce::await_code` + `gemini::exchange` + `gemini::tier` sur le runtime, puis `Event::Connected`. Après connexion réussie : `Settings { claude_auth: Subscription, .. }`, `apply_settings`, et lancement d'une tâche `Event::PlanUsage` (Claude uniquement, toutes les 5 min, silencieuse en cas d'échec).

- [ ] **Step 4: Implémenter la modale et les boutons**

Section « Abonnements » dans `settings.rs` avant « Clés API » : pour chaque fournisseur un encart avec l'état (`✅ Connecté · {account}` / `Non connecté`), les boutons « Se connecter avec Claude » (ouvre `login.url` via `open::that`, affiche le champ « Collez le code affiché après validation » + bouton « Valider »), « Se connecter avec Gemini » (ouvre l'URL, affiche « En attente de validation dans le navigateur… » + bouton Annuler), « Importer la session Claude Code » (Claude uniquement), et un sélecteur de mode (`Abonnement` / `Clé API`). La modale se ferme sur `Connected` avec un `notify` de succès ; en cas d'erreur, le message s'affiche dans la modale (jamais le code ni le jeton).

- [ ] **Step 5: Rejouer**

Run: `cargo test --lib && cargo clippy --all-targets -- -D warnings && cargo build`
Expected: PASS + binaire compilé.

- [ ] **Step 6: Vérification manuelle**

`cargo run` → ⚙ Paramètres → « Se connecter avec Gemini » → consentement Google → la modale passe à « Connecté » et le panneau droit affiche l'abonnement. Refaire avec Claude (code collé) et vérifier que le champ `claude.json` ne contient pas de jeton en clair (`Get-Content $env:APPDATA\Omega\credentials\claude.json` → une seule ligne `dpapi:…`).

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat(ui): connexion claude et gemini par abonnement"
```

---

## Self-review

- Couverture : moteur/effort (t.1), jetons (t.2), PKCE (t.3), Claude (t.4), Gemini (t.5), protocoles (t.6-7), façade+refresh (t.8), switch/utilisation (t.9), boutons de connexion (t.10) → chaque point de la demande a une tâche.
- Types : `Effort`/`Efforts`, `AuthMode`, `Cred`, `Window`, `Provider` sont définis une fois et réutilisés à l'identique dans toutes les tâches qui les citent.
- Review Focus : #1 → test tâche 2 ; #2 → fixtures tâches 6 et 7 ; #3 → test tâche 6 ; #4 → test tâche 6 ; #5 → tests tâches 6 et 8.
- Non couvert volontairement : aucun autre fournisseur, aucun stockage de clé tiers, aucune facturation.
