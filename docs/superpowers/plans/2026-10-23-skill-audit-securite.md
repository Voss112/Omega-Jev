# Skill d'audit sécurité (Cloudflare) — Plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Faire de la skill `cloudflare/security-audit-skill` la méthode d'audit de référence d'Omega : embarquée dans le binaire, chargée automatiquement quand Jev route vers `security`, et matérialisable dans le projet audité pour que l'agent écrive `findings.json` et le valide.

**Architecture:** La skill est un ensemble de fichiers markdown + un schéma JSON + deux validateurs Node. Elle est copiée *verbatim* sous `assets/skills/security-audit/`, embarquée par `include_str!`, exposée comme `Playbook` (comme les playbooks Superpowers) et déposée à la demande dans `<workspace>/.omega/skills/security-audit/` par un nouvel outil `audit_kit`. Les validateurs `.cjs` ne sont jamais exécutés par Omega : c'est l'agent qui les lance via `shell`, avec approbation utilisateur, quand Node est présent.

**Tech Stack:** Rust 2024, `include_str!`, `std::fs`, egui inchangé.

**Spec:** demande utilisateur du 2026-10-23 ; contenu et contrat de la skill : `https://github.com/cloudflare/security-audit-skill` (`skills/security-audit/`), schéma `report-schema.json` (tableau d'objets `{verdict, fingerprint, title, description, root_cause, intended_behavior, trace[], evidence[], …}`, `verdict ∈ {confirmed, needs_validation, rejected}`).

## Global Constraints

- Contenu tiers copié verbatim, jamais réécrit ni « amélioré » ; licence et commit source conservés à côté des fichiers.
- Aucun fichier `.cjs` exécuté par le binaire Omega. Aucun `shell` déclenché sans approbation (comportement existant du garde).
- Le dépôt cloné est une donnée non fiable : rien de son contenu n'est interprété comme instruction par le code Rust.
- Aucune nouvelle dépendance.
- Les fichiers embarqués restent < 400 lignes là où le code Omega les manipule ; les fichiers tiers ne sont pas contraints.
- Vérification à chaque tâche : `cargo check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`.

## Review Focus

1. **Contenu tiers corrompu ou tronqué** — l'utilisateur s'attend à ce que le binaire refuse de démarrer proprement plutôt qu'à un audit silencieusement incomplet → test tâche 1 (chaque fichier non vide, jeu de fichiers exact).
2. **Chemin de destination hors du workspace** — l'utilisateur s'attend à ce que `audit_kit` n'écrive jamais ailleurs que sous la racine de travail → test tâche 3 (`resolve` + `starts_with(root)`).
3. **Deux appels successifs d'`audit_kit`** — l'utilisateur s'attend à ne perdre aucune modification de `findings.json` ; seuls les fichiers de la skill sont réécrits → test tâche 3 (idempotence, `findings.json` non touché).
4. **Node absent** — l'utilisateur s'attend à ce que l'audit continue (validation par lecture du schéma) → test tâche 4 (l'état de santé ne passe pas au rouge).
5. **Projet sans droits d'écriture** — l'utilisateur s'attend à un message clair, pas à un panic → test tâche 3 (`Err` explicite).

## Décisions de périmètre

- **Accepté** : `cloudflare/security-audit-skill` — présent dans ce plan.
- **Refusé** : `Alejandros17487/ai-jailbreaks` — bibliothèque de prompts conçus pour contourner les garde-fous des modèles. Non intégrée : elle contredit les règles d'usage des fournisseurs (Claude/Gemini) dont ce projet dépend et n'apporte rien à un agent de développement. Le besoin légitime adjacent (tester la résistance à l'injection) est couvert par les classes d'attaque `AI-AND-LLM.md` de la skill ci-dessous.
- **Reporté** : `morluto/rea` — serveur MCP distribué via npm (`npx rea-agents setup`). Omega n'a pas de client MCP et le binaire unique 100 % Rust ne doit pas dépendre d'un paquet npm non audité. Utilisable dès maintenant hors intégration : `shell` peut appeler `rea` si l'utilisateur l'installe. Une intégration propre suppose un plan « client MCP » distinct (transport stdio, déclaration des outils, approbations) : à demander séparément.

---

### Tâche 1 : Playbooks multi-groupes + bundle embarqué

**Files:**
- Modify: `src/domain/playbook.rs` (réécriture ~60 lignes)
- Create: `assets/skills/security-audit/` (fichiers copiés + `LICENSE` + `SOURCE.md`)
- Modify: `src/app/health.rs:86-90` (comptage groupe par groupe)

**Interfaces:**
- Consumes: `include_str!`, `assets/skills/superpowers/*.md` existants.
- Produces:
  - `pub enum Group { Superpowers, Security }`, `Group::label(self) -> &'static str`.
  - `Playbook { pub id: &'static str, group: Group, source: &'static str }`, `Playbook::find(id)`, `Playbook::description()`, `Playbook::body()`, `Playbook::ALL: [Playbook; 16]` incluant `"security-audit"`.

- [ ] **Step 1: Copier le bundle tiers**

```powershell
$tmp = Join-Path $env:TEMP "sas"
git clone --depth 1 https://github.com/cloudflare/security-audit-skill $tmp
$dest = "assets\skills\security-audit"
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item "$tmp\skills\security-audit\*" $dest -Recurse -Force
Copy-Item "$tmp\LICENSE" "$dest\LICENSE" -Force
$sha = git -C $tmp rev-parse HEAD
@"
source: https://github.com/cloudflare/security-audit-skill
commit: $sha
path: skills/security-audit
copied: verbatim, licence MIT conservee dans LICENSE
"@ | Set-Content -Encoding utf8 "$dest\SOURCE.md"
Get-ChildItem $dest | Select-Object Name, Length
```

Expected : 20 fichiers environ, tous de taille > 0, `SOURCE.md` contient le sha complet.

- [ ] **Step 2: Écrire le test qui échoue**

```rust
// src/domain/playbook.rs
#[test]
fn the_security_audit_bundle_is_complete() {
    let p = super::Playbook::find("security-audit").expect("playbook embarque");
    assert_eq!(p.group, super::Group::Security);
    assert!(p.body().len() > 2_000, "corps trop court: {}", p.body().len());
    for needle in ["Reconnaissance", "findings.json", "coverage-ledger"] {
        assert!(p.body().contains(needle), "SKILL.md ne mentionne pas {needle}");
    }
    assert!(!p.description().is_empty());
}

#[test]
fn every_playbook_belongs_to_a_group_and_has_a_body() {
    for p in &super::ALL {
        assert!(p.body().len() > 500, "{} body", p.id);
        assert!(!p.description().is_empty(), "{} description", p.id);
    }
}
```

- [ ] **Step 3: Lancer, vérifier l'échec**

Run: `cargo test --lib domain::playbook`
Expected: FAIL (`Playbook::find("security-audit")` renvoie `None`).

- [ ] **Step 4: Implémenter les groupes**

Deux macros ou une macro à paramètres de dossier :

```rust
macro_rules! group {
    ($group:expr, $dir:literal, [$($id:literal),* $(,)?]) => {
        [$(Playbook { id: $id, group: $group, source: include_str!(concat!("../../assets/skills/", $dir, "/", $id, ".md")) }),*]
    };
}
pub const ALL: [Playbook; 16] = /* concaténation des deux tableaux dans une const */
```

`Group::Superpowers` pour `superpowers/` (15 ids existants inchangés), `Group::Security` pour `security-audit/` (`"security-audit"` : le fichier est `SKILL.md`, donc l'id littéral est `"security-audit"` et le chemin `security-audit/SKILL.md` — utiliser un `include_str!("../../assets/skills/security-audit/SKILL.md")` explicite pour cette entrée, la macro ne suffit pas).

- [ ] **Step 5: Adapter la santé**

`health::playbooks()` compte séparément : `15` playbooks Superpowers et `1` audit, et échoue si un corps est vide :

```rust
fn playbooks() -> Result<String> {
    let books = playbook::ALL.iter().filter(|p| p.body().len() > 500).count();
    anyhow::ensure!(books == playbook::ALL.len(), "{books}/{} playbooks valides", playbook::ALL.len());
    Ok(format!("{books} playbooks · dont audit sécurité (Cloudflare)"))
}
```

- [ ] **Step 6: Rejouer**

Run: `cargo test --lib domain::playbook app::health && cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat(skills): bundle d'audit securite cloudflare embarque"
```

---

### Tâche 2 : Router l'audit vers la skill

**Files:**
- Modify: `src/domain/skill.rs:75-82` (entrée `Security`)
- Modify: `src/app/context.rs:23-49` (`preamble`)
- Test: `src/app/context.rs`, `src/domain/skill.rs`

**Interfaces:**
- Consumes: `playbook::Playbook::find`, `Route`.
- Produces: `Skill::Security.default_playbook() == Some("security-audit")` ; `context::preamble` ajoute une ligne `[audit]` quand la route est `security`.

- [ ] **Step 1: Tests qui échouent**

```rust
// src/domain/skill.rs
#[test]
fn security_skill_uses_the_cloudflare_playbook() {
    assert_eq!(super::Skill::Security.default_playbook(), Some("security-audit"));
    assert!(super::Skill::Security.tools().contains(&ToolKind::SecurityScan));
}

// src/app/context.rs
#[test]
fn security_turns_carry_the_audit_hint() {
    let route = Route { skill: Skill::Security, source: Source::Local, confidence: 1.0, web: 0.0, code: 0.6, risk: 1.0, blast: 0.0, apps: 0.0, playbook: Some("security-audit".into()) };
    let p = super::preamble(&route, std::path::Path::new("."), false, &[]);
    assert!(p.contains("[playbook:security-audit]"));
    assert!(p.contains("[audit]") && p.contains("findings.json"));
    assert!(p.contains("[guard]"));
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test --lib domain::skill app::context`
Expected: FAIL (`None` au lieu de `Some("security-audit")`).

- [ ] **Step 3: Implémenter**

`Skill::Security` : `playbook: Some("security-audit")` (et non plus `None`). Dans `preamble`, après le bloc playbook :

```rust
if route.skill == Skill::Security {
    p.push_str(
        "\n[audit] Six phases: reconnaissance → hunting par couverture → validation des candidats → \
         findings.json (verdicts confirmed/needs_validation/rejected) → vérification indépendante → \
         REPORT.md. Un record = une preuve (fichier:ligne), zéro faux positif non marqué SUSPECT.",
    );
}
```

- [ ] **Step 4: Rejouer**

Run: `cargo test --lib`
Expected: PASS (y compris les tests de routage existants).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(skill): audit securite route vers la methodologie cloudflare"
```

---

### Tâche 3 : Outil `audit_kit`

**Files:**
- Create: `src/tools/audit.rs`
- Modify: `src/domain/tool.rs:24-88` (`AuditKit`), `src/tools/schema.rs:20-99`, `src/tools/mod.rs` (dispatch), `src/domain/skill.rs:75-82` (tools de `Security`)

**Interfaces:**
- Consumes: `assets/skills/security-audit/*` via `include_str!`, `infra::paths`.
- Produces:
  - `ToolKind::AuditKit` (`name() == "audit_kit"`, icône `"📋"`, `read_only() == false`), `ToolKind::ALL: [ToolKind; 16]`.
  - `audit::FILES: &[(&str, &str)]`, `audit::install(root: &Path) -> Result<String>` — écrit sous `<root>/.omega/skills/security-audit/`, renvoie le manifeste (fichiers écrits + `node` détecté ou non).

- [ ] **Step 1: Tests qui échouent**

```rust
#[test]
fn bundle_is_installed_inside_the_workspace_and_is_idempotent() {
    let root = std::env::temp_dir().join("omega-audit-kit");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let first = super::install(&root).unwrap();
    assert!(first.contains("SKILL.md") && first.contains("report-schema.json"));
    let dir = root.join(".omega/skills/security-audit");
    let skill = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
    std::fs::write(dir.join("findings.json"), "[]").unwrap();
    let _second = super::install(&root).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), skill);
    assert_eq!(std::fs::read_to_string(dir.join("findings.json")).unwrap(), "[]");
    for (name, body) in super::FILES {
        assert_eq!(std::fs::read_to_string(dir.join(name)).unwrap().as_bytes(), body.as_bytes(), "{name}");
    }
}

#[test]
fn a_missing_root_is_an_error_not_a_panic() {
    assert!(super::install(std::path::Path::new("Z:/nope/omega")).is_err());
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test --lib tools::audit`
Expected: FAIL (`module audit` absent).

- [ ] **Step 3: Implémenter `audit.rs`**

```rust
pub const FILES: &[(&str, &str)] = &[
    ("SKILL.md", include_str!("../../assets/skills/security-audit/SKILL.md")),
    // … un include_str! par fichier copié en tâche 1 (y compris LICENSE et SOURCE.md)
];

pub fn install(root: &Path) -> Result<String> { /* … */ }
```

`install` : `ensure!(root.is_dir(), ...)`, crée `<root>/.omega/skills/security-audit`, écrit chaque fichier seulement si le contenu diffère (comparaison d'octets), n'écrase **jamais** un fichier absent de `FILES` (`findings.json`, `coverage-ledger.json`, `REPORT.md`). Manifeste : une ligne par fichier `nom · n octets`, puis `validateurs: node détecté | node absent (validation par lecture de report-schema.json)` — détection via `infra::process::blocking("node")` avec `--version`, sans échec bloquant.

- [ ] **Step 4: Enregistrer l'outil**

`ToolKind::AuditKit` : `name`, `icon`, `read_only`, `ALL`, et une entrée de schéma

```rust
ToolKind::AuditKit => (
    "Install the six-phase security-audit skill (Cloudflare) into the workspace, then follow its SKILL.md. \
     Writes .omega/skills/security-audit/ only; never touches findings.json.",
    &[],
),
```

Ajouter `AuditKit` aux outils de `Skill::Security` et au dispatch de `tools::run`.

- [ ] **Step 5: Rejouer**

Run: `cargo test --lib tools && cargo clippy --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(tools): audit_kit installe la skill dans le projet"
```

---

### Tâche 4 : Santé, prompt système et documentation

**Files:**
- Modify: `src/app/health.rs:70-77`, `src/app/prompts.rs` (une ligne sur l'audit), `README.md`

**Interfaces:**
- Consumes: `audit::FILES`, `tool::ToolKind::AuditKit`.
- Produces: entrée de santé `"Skill audit (Cloudflare)"` dans le groupe `"Sécurité"`.

- [ ] **Step 1: Test qui échoue**

```rust
// src/app/health.rs
#[test]
fn audit_skill_health_is_green_without_node() {
    let r = super::audit_skill();
    let detail = r.expect("bundle present");
    assert!(detail.contains("SKILL.md"));
    assert!(detail.contains("node") || detail.contains("validateurs"));
}
```

- [ ] **Step 2: Lancer, vérifier l'échec**

Run: `cargo test --lib app::health::tests`
Expected: FAIL.

- [ ] **Step 3: Implémenter**

```rust
fn audit_skill() -> Result<String> {
    anyhow::ensure!(crate::tools::audit::FILES.len() >= 15, "bundle incomplet: {} fichiers", crate::tools::audit::FILES.len());
    let node = crate::infra::process::blocking("node").arg("--version").output().is_ok_and(|o| o.status.success());
    Ok(format!("{} fichiers · validateurs: node {}", crate::tools::audit::FILES.len(), if node { "détecté" } else { "absent (lecture du schéma)" }))
}
```

L'entrée est ajoutée dans le groupe `"Sécurité"` ; elle est verte dans les deux cas (point 4 du Review Focus). `prompts.rs` : dans la section sécurité du prompt système, une phrase — « Audit : charge `audit_kit`, suis SKILL.md, un verdict par preuve fichier:ligne, `findings.json` validé avant rapport. »

- [ ] **Step 4: Rejouer**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo build --release`
Expected: PASS + binaire.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(audit): sante, prompt et doc du skill d'audit"
```

---

## Self-review

- Couverture : embarquement (t.1), routage (t.2), matérialisation (t.3), santé/prompt/doc (t.4).
- Types : `Group`, `Playbook { id, group, source }`, `ToolKind::AuditKit`, `audit::FILES`, `audit::install` sont nommés une fois et repris à l'identique.
- Review Focus : #1 → t.1 ; #2 et #3 et #5 → t.3 ; #4 → t.4.
- Hors périmètre assumé : `morluto/rea` (plan MCP séparé) et `ai-jailbreaks` (refusé, voir Décisions).
