# Audit — abonnements Claude/Gemini + skill d'audit Cloudflare

Date : 2026-10-23 · Version : 0.2.0 · Périmètre : tout le diff depuis `7062c5a` (baseline).

## Passes

1. **Scan statique** (`security_scan` sur `src/`, puis sur chaque nouveau dossier) : 63 fichiers, 8 HIGH / 5 MEDIUM / 45 LOW.
2. **Revue manuelle du diff** : chaque HIGH/MEDIUM confirmé ou écarté par lecture du code ; recherche de fuites de jetons, de contournement TLS, d'injections et de courses.
3. **Vérification dynamique** : `cargo test` (51 tests), `cargo clippy --all-targets -- -D warnings`, `cargo build --release`.
4. **Contrôle des secrets sur disque** : `settings.toml` et `credentials/*.json` (test `no_api_key_is_ever_written_in_plaintext`).
5. **Empaquetage** : `installer/build-msi.ps1` → `Omega-0.2.0-x64.msi`, propriétés MSI relues via l'API Windows Installer.

## Constats corrigés

| # | Gravité | Constat | Correctif |
|---|---|---|---|
| 1 | **Critique** | `store.rs` ne scellait que `kimi/deepseek/jev` : les clés API Claude et Gemini (mode clé API) partaient **en clair** dans `settings.toml`, alors que la doc du module promettait l'inverse | `sealed()` scelle les cinq clés ; test de non-régression sur le TOML sérialisé |
| 2 | **Important** | `Cred` dérivait `Debug` : un `{:?}`, un panic ou un log aurait imprimé `access`/`refresh` | `Debug` manuel, jetons remplacés par `<scellé>` ; test `debug_never_prints_tokens` |
| 3 | **Important** | Deux tâches concurrentes (chat + relevé d'usage) pouvaient rafraîchir le même jeton en parallèle ; Anthropic/Google font tourner le refresh token → un des deux appels brûlait la session | `tokio::sync::Mutex` dans `Auth::fresh` + re-vérification après acquisition |
| 4 | Moyen | `models()` ne vérifiait pas HTTPS (seul `send()` le faisait) : une URL `http://` héritée aurait envoyé la clé en clair | garde HTTPS en tête de `models()` |
| 5 | Moyen | `audit_kit` écrivait dans le workspace sans passer par le flux d'approbation | approbation `Edit` quand « Valider chaque modification » est actif, comme les autres écritures |
| 6 | Moyen | Premier message Gemini sans projet Code Assist (onboarding fait plus tard) → erreur au premier envoi | `gemini_connect` attache le projet dès la connexion ; `ping` le persiste ; `body()` relit le projet stocké |
| 7 | Faible | `system: []` envoyé à Anthropic alors que le champ est optionnel ; certains comptes refusent un tableau vide | champ omis quand vide |

## Constats écartés (faux positifs vérifiés)

- `rust-raw-memory` / `rust-unsafe` dans `src/infra/secret.rs` : code DPAPI préexistant, borne `cbData` fournie par l'API, `LocalFree` unique, précédé d'un test de succès.
- `code-eval` sur `src/tools/mod.rs` : simple `match` de dispatch d'outils, pas d'évaluation.
- `tools/security.rs` (regex CWE-295/502-78/942, clé AWS factice) : ce sont les **définitions des règles** du scanner et son jeu d'essai, pas du code exécutable.
- `rust-panic` LOW : uniquement dans des modules `#[cfg(test)]`, sauf `SystemRandom::fill(..).expect("CSPRNG")` (échec du CSPRNG = abandon immédiat, volontaire).
- `pkce::await_code` : écoute `127.0.0.1` uniquement, valide `state` avant tout échange, ne renvoie aucun jeton au navigateur.

## Risques résiduels (acceptés)

- **OAuth tiers** : Anthropic et Google ne prévoient pas l'usage de leurs identifiants clients publics par un tiers. Suspension de compte possible : c'est l'utilisateur qui décide de connecter son abonnement.
- **Constantes Claude** (`client_id`, endpoint de jeton, en-tête `anthropic-beta`) issues du client officiel mais non documentées publiquement : peuvent changer sans préavis. Repli disponible : import de `~/.claude/.credentials.json`.
- `gemini_key`/`claude_key` en mode clé API restent en mémoire déchiffrés (nécessaire pour signer les requêtes).
- MSI non signé (pas de certificat de signature de code) : SmartScreen peut afficher un avertissement.
- Le relevé d'usage Claude dépend d'un endpoint interne (`/api/oauth/usage`) : absent ou en erreur → section masquée, jamais d'échec du tour.
- `rand` : `CSPRNG` en échec = panic volontaire plutôt qu'un vérificateur PKCE prévisible.

## Empreinte

```
Omega-0.2.0-x64.msi  SHA256 EED09929D0E625E31D87126ABEEC0DF2F4F41A936EC808F335171CBEDE7D3E44
omega.exe            FileVersion 0.2.0
```
