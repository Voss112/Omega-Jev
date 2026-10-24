# Optimisation maximale, vérification des outils, audit et MSI — Plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Réduire CPU/RAM/latence d'Omega, pousser l'économie de tokens au maximum (RTK, TOON, cache, outils différés, déduplication), déléguer davantage à Jev/DeepSeek Flash pour alléger l'abonnement Claude, vérifier chaque outil, auditer en plusieurs passes et livrer `Omega-0.6.0-x64.msi`.

**Architecture:** Changements locaux dans les couches existantes : `ui/` (cadence d'affichage, caches bornés), `infra/` (HTTP keep-alive, préchauffage, timeout Jev), `tools/` (index BM25 pré-tokenisé, TOON tabulé, grep groupé, filtres RTK par commande, lecture dédupliquée), `infra/llm/anthropic.rs` (points de cache, outils différés), `app/` (routage ∥ compaction, effort auto, délégation). Aucune réorganisation de modules.

**Tech Stack:** Rust 2024, eframe/egui 0.37, tokio, reqwest 0.13, tree-sitter 0.27, grep-searcher, WiX 3.14.

**Spec:** demande utilisateur du 2026-10-24 (ce fil). Références vérifiées : TOON spec §6/§11 (délimiteur tab : en-tête `[N<TAB>]`, champs `{a<TAB>b}`) — https://raw.githubusercontent.com/toon-format/spec/main/SPEC.md ; prompt caching (4 breakpoints max, lookback 20 blocs, `"ttl": "1h"`, 1h avant 5m) et mid-conversation tool changes (`defer_loading`, `tool_addition`/`tool_reference`, en-tête `mid-conversation-tool-changes-2026-07-01`) — docs.claude.com.

## Global Constraints

- Edition 2024 ; `cargo clippy --all-targets -- -D warnings` propre ; pas d'`unwrap` hors tests ; fichiers < 400 lignes.
- **Aucune nouvelle dépendance** (tout est faisable avec `Cargo.toml` actuel).
- UI, erreurs, libellés en français, style télégraphique existant.
- Historique filaire **append-only** préservé (cache KV + raisonnement signé Claude) : aucun changement ne réécrit un message déjà envoyé.
- Jev ne peut que **baisser** l'effort choisi par l'utilisateur, jamais le monter.
- Tout envoi de contenu vers un fournisseur autre que le moteur choisi (délégation DeepSeek) est derrière un réglage visible.
- Vérification à chaque tâche : `cargo check`, `cargo test`, `cargo clippy --all-targets -- -D warnings` ; commit par tâche.
- Tests réseau réels : `#[ignore]`, lancés explicitement en tâche 10.

## Review Focus

1. **Streaming pendant que la fenêtre n'a pas le focus** → le texte doit continuer de s'afficher à 30 FPS → test `pace` tâche 1 (`busy` non focalisé ⇒ 33 ms).
2. **Fichier modifié hors Omega entre deux `read_file`** (éditeur externe, `git checkout`) → il doit être relu, pas remplacé par « inchangé » → test tâche 6 `a_modified_file_is_read_again`.
3. **Commande en échec passant par un filtre RTK spécialisé** → chaque erreur et le code de sortie restent visibles → test tâche 5 `a_failing_cargo_build_keeps_every_error`.
4. **Compte/modèle Claude qui refuse une bêta (`tool_addition`, TTL 1h)** → le tour aboutit via repli, pas une 400 affichée → test tâche 8 `a_rejected_beta_falls_back_to_classic_tools` + passage live tâche 10.
5. **Tour délégable alors que DeepSeek est en panne ou que le message contient des images** → reste sur / retombe sur Claude → tests tâche 9 `delegable_requires_a_simple_toolless_turn` et `a_failed_delegation_reruns_on_the_main_engine`.

---

### Tâche 0 : Référence et mesures de base

**Files:**
- Create: `tools/perf.ps1`
- Create: `docs/audits/2026-10-24-mesures.md`

**Interfaces:** Produces : chiffres « avant » comparés en tâche 12.

- [ ] **Step 1: Commiter l'état courant** (beaucoup de fichiers modifiés depuis 0.3.1) : `git add -A; git commit -m "chore: base 0.5.0 avant optimisation"`.
- [ ] **Step 2: Écrire `tools/perf.ps1`** : lance `target\release\omega.exe`, attend 10 s, échantillonne `Get-Process omega` toutes les 1 s pendant 30 s (fenêtre au premier plan puis minimisée via `Add-Type` user32 `ShowWindow`), affiche CPU moyen (%), Working Set max (Mo), puis arrête le processus. `$ErrorActionPreference='Stop'`.
- [ ] **Step 3: Mesurer** : `cargo build --release; .\tools\perf.ps1`, plus `cargo test --release docs_search_timing -- --ignored --nocapture` (ajouté tâche 3 ; noter « n/a » ici). Reporter dans `docs/audits/2026-10-24-mesures.md` (colonnes : métrique, avant, après).
- [ ] **Step 4: Commit** `chore: script de mesure perf`.

---

### Tâche 1 : UI — cadence d'affichage et caches bornés

**Files:**
- Create: `src/ui/pace.rs`
- Modify: `src/ui/mod.rs:283-288,348-351` (cadence), `src/ui/mod.rs:79-134` (champ `tex_owner`)
- Modify: `src/ui/chat.rs:171-176` (éviction des textures au changement de conversation)

**Interfaces:**
- Produces: `pub struct Activity { pub focused: bool, pub minimized: bool, pub busy: bool, pub animating: bool, pub timers: bool }` ; `pub fn next_frame(a: Activity) -> Option<Duration>` ; `attach::image_keys(conv: &Conversation, shots: &[Image]) -> HashSet<u64>`.

- [ ] **Step 1: Tests** dans `pace.rs` :

```rust
#[test]
fn frame_pacing_follows_activity() {
    let a = Activity { focused: true, minimized: false, busy: false, animating: false, timers: false };
    assert_eq!(next_frame(a), Some(Duration::from_millis(250)));
    assert_eq!(next_frame(Activity { busy: true, focused: false, ..a }), Some(Duration::from_millis(33)));
    assert_eq!(next_frame(Activity { animating: true, ..a }), Some(Duration::from_millis(33)));
    assert_eq!(next_frame(Activity { focused: false, ..a }), None);
    assert_eq!(next_frame(Activity { focused: false, timers: true, ..a }), Some(Duration::from_secs(1)));
    assert_eq!(next_frame(Activity { minimized: true, busy: true, ..a }), None);
    assert_eq!(next_frame(Activity { minimized: true, timers: true, ..a }), Some(Duration::from_secs(1)));
}
```

et dans `attach.rs` : `image_keys_cover_the_conversation_and_the_composer` (1 image dans un message + 1 dans `shots` ⇒ 2 clés, égales à `key(img)`).
- [ ] **Step 2:** `cargo test frame_pacing image_keys` → FAIL (non défini).
- [ ] **Step 3: Implémenter.** `next_frame` : minimisé ⇒ `timers` ? 1 s : `None` ; `busy || animating` ⇒ 33 ms ; focalisé ⇒ 250 ms ; sinon `timers` ? 1 s : `None`. Dans `App::ui` : `animating = login.is_some() || bench_running || !hovered_files.is_empty()`, `timers = notice.is_some() || !jobs.is_empty()` ; remplace les trois `request_repaint_after` actuels par un seul. Les événements du runtime réveillent déjà l'UI (`Emitter` → `request_repaint`). Textures : champ `tex_owner: Option<Uuid>` ; quand `current != tex_owner`, `tex.retain(|k, _| keep.contains(k))` avec `image_keys`.
- [ ] **Step 4:** tests PASS ; `cargo clippy --all-targets -- -D warnings`.
- [ ] **Step 5: Commit** `perf(ui): cadence adaptative, textures évincées`.

---

### Tâche 2 : Réseau — keep-alive, préchauffage, Jev rapide, routage ∥ compaction

**Files:**
- Modify: `src/infra/http.rs:22-37`
- Modify: `src/infra/llm/mod.rs` (ajout `warm`), `src/infra/jev.rs:90-114`
- Modify: `src/app/router.rs:39-109`, `src/app/agent.rs:35-40`, `src/ui/mod.rs` (appel de préchauffage après `build_env`)

**Interfaces:**
- Produces: `Llm::warm(&self)` ; `Jev::warm(&self)` ; `Jev::ask(&self, state: Value, questions: &[(String, Question)], timeout: Duration) -> Result<Answers>` ; `pub const ROUTE_TIMEOUT: Duration = Duration::from_secs(6)` (jev.rs) ; `router::Input { query: String, previous: Option<String>, turns: usize, last_route: Option<Route> }`, `Input::of(conv: &Conversation) -> Input`, `router::route(env: &Env, input: &Input) -> (Route, u64)`.

- [ ] **Step 1: Test** `router.rs` : `a_bare_follow_up_reuses_the_previous_route` (conversation : user « audit » avec `route` Security, assistant, user « continue » ⇒ `Input::of` porte `last_route` Security ; `route` sans Jev renvoie Security, 0 token). Lancer → FAIL.
- [ ] **Step 2: Implémenter.** `blast_radius ask` avant de changer la signature (appelants : router, shell, health, bench). HTTP : `.pool_idle_timeout(300 s)`, `.tcp_keepalive(60 s)`, `.http2_keep_alive_interval(30 s)`, `.http2_keep_alive_while_idle(true)` (vérifier les noms dans `deps/reqwest-0.13*/src/async_impl/client.rs` avant usage). `warm` = `HEAD` sur la base HTTPS, timeout 5 s, erreur ignorée, seulement si `configured()` / Jev présent. `agent.rs` : `let input = router::Input::of(conv); let (_, (route, jev_tokens)) = tokio::join!(compact::maybe(conv, env, emit), router::route(env, &input));`.
- [ ] **Step 3:** tests PASS, clippy propre.
- [ ] **Step 4: Commit** `perf(net): keep-alive http2, préchauffage, jev 6 s, routage parallèle`.

---

### Tâche 3 : Index web — BM25 pré-tokenisé et plafond mémoire

**Files:**
- Modify: `src/tools/webindex.rs` (tout le fichier)

**Interfaces:**
- Produces: `struct Doc { len: u32, tf: HashMap<Box<str>, u16> }` ; `fn doc(text: &str) -> Doc` ; `fn rank(query: &str, docs: &[&Doc], k: usize) -> Vec<(usize, f32)>` ; `pub fn bm25(query, docs: &[&str], k)` conservée (clihub) = `doc` + `rank` ; `fn evict(pages: &mut Vec<Indexed>, max_pages: usize, max_bytes: usize)` ; `const MAX_BYTES: usize = 48 << 20`.

- [ ] **Step 1: Tests** : `indexed_search_ranks_like_plain_bm25` (3 chunks fixes, requête « rust async » : même ordre et scores à 1e-4 près que l'ancienne implémentation copiée dans le test) ; `the_index_evicts_oldest_pages_past_the_byte_cap` (`evict` avec `max_bytes = 10`, trois pages de 6 octets ⇒ reste la plus récente) ; `#[ignore] docs_search_timing` (1000 pages × 80 chunks synthétiques, imprime la durée de `search`).
- [ ] **Step 2:** FAIL.
- [ ] **Step 3: Implémenter.** `pages: RwLock<Vec<Indexed>>` avec `Indexed { page: Page, docs: Vec<Doc> }` construits à `open`/`put` ; `search` ne tokenise que la requête ; `evict` appliqué à `open` et `put` (réécriture `write_all` si éviction).
- [ ] **Step 4:** PASS ; `cargo test --release docs_search_timing -- --ignored --nocapture` < 50 ms. Noter avant/après dans `mesures.md`.
- [ ] **Step 5: Commit** `perf(web): index bm25 pré-tokenisé, plafond 48 Mo`.

---

### Tâche 4 : TOON tabulé et grep groupé par fichier

**Files:**
- Modify: `src/tools/compress.rs:114-127,136-144`
- Modify: `src/tools/search.rs`

**Interfaces:**
- Produces: `toon(name, fields, rows)` émet `name[N\t]{f1\tf2}:` puis lignes `  v1\tv2` ; `grep` émet le format ci-dessous.

- [ ] **Step 1: Tests** :

```rust
#[test]
fn toon_rows_are_tab_separated_and_commas_stay_raw() {
    let out = toon("m", &["a", "b"], &[vec!["x, y".into(), "f(\"z\")".into()]]);
    assert_eq!(out, "m[1\t]{a\tb}:\n  x, y\tf(\"z\")");
}
#[test]
fn grep_groups_hits_by_file() { /* 2 fichiers, 3 hits */
    // attendu exact :
    // "matches 3 in 2 files\na.txt\n  1\tneedle\n  3\tneedle\nb.txt\n  2\tneedle"
}
```

Adapter `search::self_test` (`"probe.txt\n  2\tneedle_42"`) et `toon_self_test`. Quoting `cell` : seulement si vide, contient `\t` ou `\n`, commence/finit par un espace, ou commence par `"` (écart volontaire au §7.2 : lecteur = LLM ; documenté en commentaire d'une ligne).
- [ ] **Step 2:** FAIL.
- [ ] **Step 3: Implémenter.** Tronqué : 1re ligne `matches {shown}/{total} in {files} files`, ligne finale `(refine pattern or glob)` conservée.
- [ ] **Step 4:** `cargo test` complet (security/graph/code utilisent `toon` : corriger leurs assertions) → PASS.
- [ ] **Step 5: Commit** `perf(tokens): toon tabulé, grep groupé`.

---

### Tâche 5 : RTK par commande

**Files:**
- Create: `src/tools/rtk.rs`
- Modify: `src/tools/compress.rs:59-63`, `src/tools/shell.rs:62-92`, `src/tools/mod.rs:3-18`, `src/app/bench.rs:176,303`

**Interfaces:**
- Produces: `rtk::specialize(cmd: &str, text: &str) -> Option<String>` ; `compress::command(cmd: &str, raw: &str) -> String` (= `specialize` puis filtre générique ; remplace `compress::shell`) ; `run_command(command, dir, timeout_secs, hint: &str)`.

- [ ] **Step 1: Tests** dans `rtk.rs` (fixtures en dur) :
  - `cargo_warnings_are_grouped` : 12 × `warning: unused variable` ⇒ 3 blocs gardés + `… 9 more: unused variable` ; lignes `= note: \`#[warn(...)]\` on by default` supprimées.
  - `a_failing_cargo_build_keeps_every_error` : 4 `error[E…]` distincts ⇒ les 4 présents en entier.
  - `git_status_hints_are_dropped` : lignes `(use "git …")` absentes, chemins présents.
  - `git_diff_keeps_hunks_and_one_header` : `diff --git`, `index …`, `--- a/` supprimées ; `+++ b/x` et hunks gardés.
  - `pytest_keeps_failures_only` : sortie avec `= FAILURES =` ⇒ commence à cette section ; sortie verte ⇒ seule la ligne de résumé `N passed`.
  - `npm_install_keeps_errors_and_summary` : garde `ERR!`, `added N packages`, `vulnerabilities`.
  - `unknown_command_falls_back_to_generic` : `specialize("echo hi", …) == None`.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3: Implémenter.** Détection sur le dernier segment de `cmd` (après `;`, `&&`, `|`) : `cargo (build|check|clippy|test|run)`, `git status`, `git diff`, `pytest`/`python -m pytest`, `(npm|pnpm|yarn) (install|i|ci|add)`. `blast_radius run_command` avant changement. Économie comptée comme aujourd'hui (`SAVED`).
- [ ] **Step 4:** PASS, clippy propre.
- [ ] **Step 5: Commit** `perf(tokens): filtres rtk par commande`.

---

### Tâche 6 : `read_file` dédupliqué

**Files:**
- Create: `src/tools/reads.rs`
- Modify: `src/tools/mod.rs:32-39,78-81`, `src/app/agent.rs:71`

**Interfaces:**
- Produces: `pub struct Reads` ; `type ReadKey = (String, Option<usize>, Option<usize>)` ; `Reads::from_history(messages: &[Message]) -> Reads` ; `Reads::unchanged(&self, root: &Path, key: &ReadKey) -> Option<String>` ; `Reads::record(&self, key: ReadKey)` ; `ToolCtx.reads: &'a Reads`.

- [ ] **Step 1: Tests** (dossier temporaire) :
  - `an_unchanged_range_is_not_resent` : historique avec appel `read_file {path:"a.rs",start:1,end:9}` + résultat ⇒ `unchanged` renvoie le talon `[unchanged since your earlier read_file of a.rs 1-9; reuse that output]`.
  - `a_modified_file_is_read_again` : mtime postérieur à `at − 2 s` ⇒ `None`.
  - `an_errored_read_does_not_count` : résultat commençant par `ERROR:` ⇒ `None`.
  - `another_range_is_read` : même fichier, `start:10` ⇒ `None`.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3: Implémenter.** Clé normalisée (`\`→`/`, `./` retiré). Valeur = `at` du message outil (ou `Utc::now()` à `record`). Marge 2 s sur le mtime. `turn_loop` construit `Reads::from_history(&conv.messages[visible..])` (après compaction : les lectures résumées sont oubliées). `exec` ReadFile : `unchanged` ⇒ talon ; sinon lecture puis `record`.
- [ ] **Step 4:** PASS.
- [ ] **Step 5: Commit** `perf(tokens): read_file inchangé non renvoyé`.

---

### Tâche 7 : Cache Anthropic — 3ᵉ point et TTL 1 h

**Files:**
- Modify: `src/infra/llm/anthropic.rs:104-113`, tests `:395-410`

**Interfaces:** Produces : système `{"type":"ephemeral","ttl":"1h"}` ; fin d'historique `{"type":"ephemeral"}` ; nouveau point sur le dernier bloc du message `user` qui précède le dernier message `assistant` (fin exacte de la requête précédente).

- [ ] **Step 1: Tests** : `the_previous_request_end_carries_a_breakpoint` (user, assistant+12 tool_use, 12 tool_result ⇒ `cache_control` sur le dernier bloc du 1er user) ; `the_static_prefix_is_cached_for_an_hour` (`system[last].cache_control.ttl == "1h"`) ; `never_more_than_four_breakpoints` (compte récursif ≤ 4) ; jamais sur un bloc `thinking`/`redacted_thinking`.
- [ ] **Step 2:** FAIL. **Step 3:** implémenter. **Step 4:** PASS (+ `savings::prefix_self_test` inchangé : `strip_markers` retire déjà `cache_control`).
- [ ] **Step 5: Commit** `perf(cache): point de cache requête précédente, ttl 1h`.

---

### Tâche 8 : Outils différés Anthropic (`tools` constant + `tool_addition`)

**Files:**
- Modify: `src/domain/chat.rs:55-90` (champ `tools_added: Vec<ToolKind>`, serde default/skip)
- Modify: `src/app/agent.rs:46-58`, `src/app/context.rs:69-88`, `src/app/savings.rs:68-78`
- Modify: `src/infra/llm/anthropic.rs` (rôle canonique `tools`, `defer_loading`), `src/infra/llm/mod.rs:76-80,339-346`
- Modify: `src/infra/auth/claude.rs` (constante d'en-tête bêta)

**Interfaces:**
- Consumes: `ToolKind::READ`, `schema::schemas`.
- Produces: `anthropic::inline_tools(model: &str) -> bool` ; message canonique `{"role":"tools","add":["grep",…]}` (ignoré par le fil OpenAI) ; `llm::tools_beta_rejected(e: &anyhow::Error) -> bool` ; `Llm.deferred: Arc<AtomicBool>` (désactivé après rejet, pour la session).

- [ ] **Step 1: Vérifier** sur https://docs.claude.com/en/docs/build-with-claude/mid-conversation-system-messages la liste des modèles pris en charge et les limitations ; figer `inline_tools` dessus (familles via `anthropic::thinking`-style parsing). Si `claude-sonnet-4-5` (défaut) n'est pas pris en charge, le mode reste inactif pour lui.
- [ ] **Step 2: Tests** : `deferred_mode_keeps_the_tools_array_constant` (deux corps, toolsets différents ⇒ `tools` identiques, outils hors `READ` avec `defer_loading: true`) ; `a_turn_that_adds_tools_appends_a_tool_addition_message` (message système `[{type:"tool_addition",tool:{type:"tool_reference",name:"grep"}}]` à la position du tour) ; `openai_wire_ignores_tool_additions` ; `a_rejected_beta_falls_back_to_classic_tools` (`tools_beta_rejected` sur 400 `tool_reference_unresolved` ou mention de l'en-tête ⇒ corps classique).
- [ ] **Step 3:** FAIL.
- [ ] **Step 4: Implémenter.** `tools_added` du tour = `(route.tools() ∪ conv.toolset) − READ − ⋃ tools_added précédents` (rattrape les anciennes conversations une fois). En mode différé, `savings::prefix` utilise `ToolKind::ALL`. Repli dans `turn_loop` sur le modèle de `thinking_rejected` (une relance). En-tête `mid-conversation-tool-changes-2026-07-01` joint par virgule à `claude::BETA` (OAuth) ou ajouté seul (clé API).
- [ ] **Step 5:** PASS, clippy.
- [ ] **Step 6: Commit** `perf(cache): outils différés claude sans casser le préfixe`.

---

### Tâche 9 : Jev en plus — effort auto, délégation DeepSeek Flash, compaction déléguée

**Files:**
- Modify: `src/domain/route.rs` (champs `effort: Option<f32>`, `simple: f32`, serde default)
- Modify: `src/domain/engine.rs:13-22` (`PartialOrd, Ord` sur `Effort`)
- Modify: `src/domain/settings.rs:54-115` (`auto_effort: bool` = true, `delegate: bool` = true)
- Modify: `src/app/router.rs:59-108`, `src/app/mod.rs:40-64`, `src/app/agent.rs`, `src/app/compact.rs:35-39`
- Modify: `src/ui/settings.rs` (2 interrupteurs : « Effort ajusté par Jev (plafond = votre choix) », « Déléguer à DeepSeek Flash : questions simples et résumés »)

**Interfaces:**
- Produces: `Route::effort_cap(&self, chosen: Effort) -> Effort` ; `Route::delegable(&self) -> bool` ; `Env.helper: Option<Llm>` ; `Env::with_llm(&self, llm: Llm) -> Env`.

- [ ] **Step 1: Tests** :
  - `effort_cap_never_raises_the_users_choice` : score 0.1 + choix High ⇒ Low ; score 0.9 + choix Medium ⇒ Medium ; choix Off ⇒ Off ; `effort: None` ⇒ choix inchangé.
  - `delegable_requires_a_simple_toolless_turn` : `simple 0.9`, rien d'autre ⇒ true ; + `code 0.6` ⇒ false ; + playbook ⇒ false ; `simple 0.7` ⇒ false.
  - `old_routes_without_effort_still_load` (JSON sans les nouveaux champs).
  - `a_failed_delegation_reruns_on_the_main_engine` : extraire `fn pick(route, settings, helper_ok, has_images) -> Pick { Main, Helper }` et tester images ⇒ Main, helper absent ⇒ Main.
- [ ] **Step 2:** FAIL.
- [ ] **Step 3: Implémenter.** Questions Jev ajoutées : `effort` = `Question::Score` niveaux `["trivial","light","standard","deep"]` ; `simple` = noul « Simple chat or factual question answerable without tools, files or deep reasoning? ». Seuils effort : < 0.25 Low, < 0.5 Medium, < 0.75 High, sinon Max, puis `min(choix)`. `auto_effort` ne s'applique que si le changement d'effort ne casse pas le cache : DeepSeek, ou Claude en `Thinking::Adaptive` (jamais `Budget`, dont le changement invalide le cache messages). `helper` = `Llm::new(http, settings, Engine::DeepSeek)` si `delegate`, moteur ≠ DeepSeek et `configured()`. `compact::maybe` résume avec `helper` sinon `llm` (ledger aux tarifs du modèle réellement utilisé). Délégation : `turn_loop` exécute le tour sur `env.with_llm(helper)` ; sur erreur du helper au premier appel, relance sur le moteur principal.
- [ ] **Step 4:** PASS, clippy.
- [ ] **Step 5: Commit** `feat(jev): effort plafonné, délégation flash, résumés délégués`.

---

### Tâche 10 : Vérification de chaque outil

**Files:**
- Create: `docs/audits/2026-10-24-verification-outils.md`

- [ ] **Step 1:** `cargo test` (inclut `every_local_tool_passes_its_self_test` : tree-sitter/outline/find_symbol, graphe/blast_radius, grep, guard, RTK, TOON, scanner statique) → 0 échec.
- [ ] **Step 2:** `cargo test -- --ignored --nocapture` (réseau : `live_stream`, `clihub_live_registry_and_skill`, `bench_live` = lazy tools, RTK, TOON, cache préfixe KV, style caveman, routage Jev) ; noter sorties réelles. Si une variable (`OMEGA_TEST_MODEL`…) manque, la demander à l'utilisateur, ne rien inventer.
- [ ] **Step 3: Superpowers (obra)** : lister `https://api.github.com/repos/obra/superpowers/contents/skills` et comparer à `assets/skills/superpowers/*.md` ; toute compétence manquante ou modifiée en amont est signalée, mise à jour seulement après accord.
- [ ] **Step 4: Recherche web** : `web_search` + `web_fetch` + `docs_search` via `bench_live`/`probe` ; **CLI-Anything** : `clihub::self_check`.
- [ ] **Step 5:** Rédiger le rapport (tableau outil → commande → résultat réel → OK/KO) ; tout KO corrigé avec test avant de continuer.
- [ ] **Step 6: Commit** `docs: vérification des outils`.

---

### Tâche 11 : Audit complet multi-passes

**Files:**
- Create: `docs/audits/2026-10-24-audit-complet.md`, `docs/audits/2026-10-24-findings.json`

- [ ] **Passe 1 — statique :** `security_scan` sur `src`, `cargo clippy --all-targets -- -D warnings -W clippy::pedantic` (lecture seule des avertissements), `cargo audit` s'il est installé (sinon demander l'accord avant `cargo install cargo-audit`).
- [ ] **Passe 2 — chasse par couverture** suivant `assets/skills/security-audit/SKILL.md` : `infra/auth` (PKCE, stockage DPAPI), `infra/llm` (en-têtes, erreurs sans secret), `infra/http` (SSRF/DNS rebinding), `tools/guard`+`shell` (contournements), `tools/fs`/`guard::resolve*` (traversée), `tools/web`/`webindex` (HTML hostile, taille), `ui/attach` (bombes d'image, presse-papiers), `infra/store`/`secret`, puis tout le code des tâches 1-9 (nouvelle bêta, délégation : fuite de contenu vers DeepSeek si `delegate` désactivé).
- [ ] **Passe 3 — validation :** chaque candidat prouvé `fichier:ligne` (lecture du code) ou rejeté.
- [ ] **Passe 4 — `findings.json`** validé contre le schéma du skill (verdicts `confirmed`/`needs_validation`/`rejected`).
- [ ] **Passe 5 — vérification indépendante :** relire chaque `confirmed` à froid ; corriger avec test de non-régression.
- [ ] **Passe 6 — rapport** `audit-complet.md` ; commit `docs(audit): audit complet 0.6.0`.

---

### Tâche 12 : Version 0.6.0 et MSI

**Files:**
- Modify: `Cargo.toml:3`, `README.md` (section « Ce qui remplace quoi » : RTK par commande, TOON tab, read dédupliqué, outils différés, délégation Jev/Flash), `docs/audits/2026-10-24-mesures.md` (colonne « après »)

- [ ] **Step 1:** `version = "0.6.0"` ; `cargo build --release` ; `.\tools\perf.ps1` → compléter `mesures.md`.
- [ ] **Step 2:** `.\installer\build-msi.ps1` → attendu `MSI: …\Omega-0.6.0-x64.msi` ; vérifier `Test-Path`.
- [ ] **Step 3: Commit** `chore(release): 0.6.0 (optimisation, audit, msi)`.
