# Audit complet — Omega 0.6.0 (branche `perf/optimisation-max`)

Date : 2026-10-24 · Méthode : skill `security-audit` embarqué (six phases), appliqué à la surface modifiée par 0.6.0
et aux sorties des outils statiques sur tout `src/`. Le périmètre auth/abonnements a été audité la veille
(`2026-10-23-audit-abonnements.md`) et n'est pas re-chassé ici ; ses records ne sont pas reportés.

## Résultat

| Verdict | Nombre |
|---|---|
| confirmed | 0 |
| needs_validation | 0 |
| rejected | 8 |

`findings.json` : 8 records, validé contre `report-schema.json` + règles sémantiques (`validateDocument`) → **PASS**.
Sous Windows, `validate-findings.cjs` refuse de lire (pas de `O_NOFOLLOW`) : la validation a appelé la même fonction
exportée sur une lecture simple de notre propre fichier.

## Passe 1 — statique

- `security_scan src` : 76 fichiers, 8 HIGH, 5 MEDIUM, 125 LOW.
  - HIGH/MEDIUM : motifs et fixture du scanner lui-même, nom `exec` du répartiteur d'outils, DPAPI (`unsafe` FFI) → tous rejetés après lecture (records ci-dessous).
  - LOW : `unwrap` uniquement dans des modules `#[cfg(test)]`, URL `http://` dans les tests SSRF.
- `cargo audit` (592 crates) : **0 vulnérabilité**, 6 avertissements « unmaintained » transitifs (`bincode`, `ttf-parser`, `yaml-rust`, `zbus_names`, `zvariant`, `zvariant_derive`) — aucun avis de faille.
- `cargo clippy --all-targets -- -W clippy::pedantic` : uniquement des conversions numériques (affichage, x64), backticks de doc, styles. Aucun impact sécurité. `-D warnings` standard : propre.

## Passe 2 — chasse par couverture (surface 0.6.0)

| Frontière | Fichier | Contrôle vérifié |
|---|---|---|
| Préchauffage réseau | `ui/actions.rs`, `infra/http.rs`, `infra/jev.rs`, `infra/llm/mod.rs` | HEAD sans en-tête d'auth, https seul, même client (proxy, résolveur anti-SSRF), kill-switch VPN respecté |
| Délégation DeepSeek | `app/agent.rs`, `app/mod.rs`, `app/compact.rs` | interrupteur, clé requise, jamais d'images, même proxy |
| Messages système `tool_addition` | `app/context.rs`, `infra/llm/anthropic.rs` | uniquement des références à des noms d'outils statiques ; aucune donnée non fiable en message système |
| `read_file` dédupliqué | `tools/reads.rs` | même garde `resolve_read` que la lecture réelle |
| Index web | `tools/webindex.rs` | plafonds 1000 pages / 48 Mio, tf saturé, index proportionnel au texte borné |
| RTK par commande | `tools/rtk.rs` | regex linéaires (crate `regex`), n'influence pas la garde shell |
| grep parallèle | `tools/search.rs` | threads bornés (≤ 16), lignes gardées ≤ max par tranche |
| Recherche web | `tools/web.rs` | repli `lite`, aucun contournement du résolveur ni du proxy |

## Passes 3 à 5 — validation, records, vérification indépendante

Chaque candidat a été prouvé ou rejeté sur `fichier:ligne` ; toutes les lignes citées ont été relues à froid après
le dernier commit (une référence décalée, `web.rs:148` → `164`, corrigée). Auteur et vérificateur sont la même
session : vérification indépendante **faible**, à reprendre par un relecteur neuf si un niveau plus élevé est requis.

## Défauts non sécurité trouvés pendant la vérification

- **DuckDuckGo** : le point d'accès `html` répondait 403 et l'erreur remontait avant le repli `lite` ; la sonde de
  santé faisait un HEAD (vert alors que la recherche échouait). Corrigé (`40ed8d8`) : repli sur toute erreur, sonde
  par vraie requête, message 403/429 explicite qui oriente vers `docs_search`. Le blocage observé ensuite touchait
  aussi `curl` depuis la même IP (limitation temporaire par IP chez DuckDuckGo).
