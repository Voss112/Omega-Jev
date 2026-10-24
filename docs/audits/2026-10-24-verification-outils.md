# Vérification réelle de chaque outil — Omega 0.6.0

Date : 2026-10-24 · Paramètres enregistrés de l'utilisateur (Claude `claude-opus-5-5` par abonnement, DeepSeek
`deepseek-flash` par clé, Jev). Aucune clé affichée. Commandes lancées depuis la racine du projet.

## Résultats

| Outil / levier | Commande | Résultat réel | État |
|---|---|---|---|
| Suite locale (tree-sitter, graphe, grep, garde, RTK, TOON, scanner) | `cargo test` | 132 réussis, 0 échec, 6 ignorés (réseau) | OK |
| Moteurs IA | `cargo test --release -- --ignored health_live` | DeepSeek `deepseek-flash` clé API · Claude `claude-opus-5-5` abonnement · Jev 294 tokens/appel | OK |
| Tree-sitter AST | idem | 11/11 langages | OK |
| Graphe d'impact | idem | petgraph, profondeur 3 | OK |
| ripgrep | idem | grep-searcher + ignore (gitignore) | OK |
| Shell | idem | PowerShell | OK |
| RTK | `bench_live` | log `cargo test` 103 lignes : 1941 → 304 tokens (−84 %), erreur et résumé conservés | OK |
| TOON | `bench_live` | 30 résultats grep : JSON 1177 → TOON 859 tokens (−27 %) | OK |
| Outils à la demande (lazy) | `bench_live` | 16 outils 2456 tok · code 1599 · chat 37 → −857 tok/requête en code, −2419 en chat | OK |
| Routage Jev | `bench_live` | 7/7 justes (local 7/7), schémas envoyés −56 % | OK |
| Cache préfixe KV | `bench_live` | 2ᵉ requête : 3676/3680 tokens lus du cache (100 %) | OK |
| Outils différés `tool_addition` (nouveau) | `bench_live` | `tools` constant (9 différés), ajout de `web_search` accepté par l'API, 2ᵉ tour 2450/2595 lus du cache | OK |
| Style caveman (Ultra) | `bench_live` | réponse visible Normal 1557 → Ultra 697 tokens (−55 %) | OK |
| Superpowers (obra) | API GitHub `repos/obra/superpowers/contents/skills` + `git hash-object` | 15/15 compétences, contenus identiques à l'amont | OK |
| Skill audit (Cloudflare) | `health_live` | 20 fichiers, node détecté | OK |
| CLI-Anything (clihub) | `bench_live`, `clihub_live_registry_and_skill` | 103 CLIs · « raster image editing » → gimp · install pip sûre | OK |
| `web_fetch` | `web_tools_live` | docs.rs/tokio → 4115 car., préfixe « untrusted » présent | OK |
| `docs_search` (BM25 local) | `web_tools_live` | page tokio retrouvée (score 1,8) | OK |
| `web_search` (DuckDuckGo) | `web_tools_live` | IP limitée par DuckDuckGo (403 puis 202 captcha, `curl` bloqué aussi) | KO externe, défaut code corrigé |
| Scanner statique | `health_live` | 25 règles (CWE) | OK |
| Garde shell + sandbox | `health_live` | 10 règles bloquantes, 16 à risque | OK |
| Règles projet | `health_live` | aucun `OMEGA.md` dans le dossier de travail | Info (fichier facultatif) |

Coût du banc : 14 971 tokens moteur + 13 243 tokens Jev.

## Défauts trouvés et corrigés

1. **Repli DuckDuckGo inopérant** : un 403 sur `html` remontait avant l'essai `lite`. → Repli sur toute erreur.
2. **Sonde de santé trompeuse** : un HEAD répondait 200 alors que la recherche échouait. → La sonde lance une vraie requête.
3. **Captcha pris pour « aucun résultat »** : DuckDuckGo renvoie un 202 avec une page `anomaly-modal`, et le modèle recevait
   « no results; try other keywords ». → Détecté comme blocage, message explicite qui oriente vers `docs_search` / proxy.
   Tests : `a_throttled_search_is_reported_as_such`, `a_bot_challenge_page_is_not_an_empty_result`.

Non vérifiable ici : `live_stream` (demande `DEEPSEEK_API_KEY` en variable d'environnement, absente) ; DeepSeek est
vérifié par `health_live` via les paramètres enregistrés.
