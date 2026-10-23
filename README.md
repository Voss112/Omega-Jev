# Omega — agent IA local, anonyme, 100 % Rust

**Jev** (System One, TypeSafe) prend toutes les décisions de contrôle : skill, budget de raisonnement, web, risque, impact.
**Kimi K3.1 Pro** (effort `low` imposé) ou **DeepSeek V4.1 Flash 1017** (effort `max`), au choix dans le menu déroulant de la zone de saisie, écrit le code et exécute les outils (System Two).
Tout tient dans un seul binaire (Windows et Linux), sans serveur externe ni télémétrie.

## Démarrage

1. Windows : double-cliquer `Omega-0.3.1-x64.msi` (installation par utilisateur, sans droits admin, raccourci Bureau). Linux : `target/release/omega`.
2. Au premier lancement, la fenêtre ⚙ Paramètres s'ouvre. Coller la clé **Kimi** (platform.kimi.ai) et/ou **DeepSeek** (platform.deepseek.com) et la clé **Jev** (console.typesafe.ai ; sans elle, un routeur local prend le relais).
3. Choisir un dossier de travail (📁 en bas à gauche). Par défaut : `%APPDATA%/Omega/workspace`.

Les clés peuvent aussi venir des variables `MOONSHOT_API_KEY`, `DEEPSEEK_API_KEY` et `TYPESAFE_API_KEY`.

Règles permanentes d'un projet : `OMEGA.md` à sa racine (bouton « Créer OMEGA.md » dans le panneau Outils). `AGENTS.md` et `CLAUDE.md` sont aussi lus.

## Interface

- **Fichiers** (bouton 📁 en haut) : arborescence du dossier de travail, clic pour ouvrir un fichier dans un onglet au-dessus de la discussion, visionneuse numérotée en lecture seule (512 Ko max, binaire détecté).
- **Images** : glisser-déposer, `Ctrl+V` (capture d'écran) ou 🖼 dans la zone de saisie. Redimensionnées à 1568 px, 4 max par message, vignettes dans la bulle. Envoyées uniquement aux moteurs qui lisent les images (Claude, Gemini) ; Kimi/DeepSeek les gardent locales.
- **⏰ Programmer** : prompt + jour + heure + moteur + effort, répétition quotidienne optionnelle. À l'heure dite, Omega ouvre une discussion et l'exécute **toujours en bypass**. Les tâches sont enregistrées dans `%APPDATA%/Omega/schedules.json` et ne partent que si Omega est ouvert.
- **Dernier verdict Jev** : skill, source, confiance, cinq métriques (code, web, risque, impact, apps) et outils chargés, en cartes compactes.

## Compiler

```sh
cargo build --release                      # binaire optimisé (LTO), icône intégrée
cargo test                                 # tests outils, domaine, DPAPI, SSRF, checkpoints
powershell -File installer/build-msi.ps1   # MSI (WiX 3.x dans %USERPROFILE%\tools\wix314 ou $env:WIX_BIN)
```

Linux : paquets X11/Wayland usuels requis à l'exécution (comme toute app egui).

## Ce qui remplace quoi (in-process)

| Outil d'origine | Intégration Omega |
|---|---|
| lazy-tool | Jev choisit le skill ; seuls ses outils sont envoyés (ensemble monotone par discussion → cache stable) |
| Serena | `outline`, `find_symbol`, `replace_symbol` via tree-sitter (11 langages), édition vérifiée syntaxiquement |
| code-review-graph | `blast_radius` : graphe de références petgraph, BFS inverse profondeur 3 |
| RTK | compression des sorties shell (ANSI, bruit, tests passés, doublons, tête/queue) |
| caveman | 4 styles de sortie (Normal → Ultra-caveman) |
| Superpowers | 15 playbooks officiels (MIT) embarqués ; Jev en choisit au plus un par message, injecté une seule fois par discussion |
| Audit sécurité (Cloudflare) | skill `security-audit` embarqué verbatim (MIT, 20 fichiers, schéma `report-schema.json`) ; Jev le route sur `security`, l'outil `audit_kit` le dépose dans `<workspace>/.omega/skills/security-audit/`, les validateurs `.cjs` ne tournent que si Node est présent |
| Semgrep / mcp-scan | `security_scan` : 25 règles CWE, secrets masqués ; garde shell déterministe + avis Jev + approbation |
| CLI-Anything (HKUDS) | outil `cli_anything` : registre officiel (100+ CLIs : GIMP, Blender, LibreOffice, OBS, Zotero…), mode d'emploi SKILL.md à la demande, installation pip/npm validée par vous, exécution `--json` sans shell |
| Recherche web | DuckDuckGo anonyme, tri des résultats par Jev Score, pages en Markdown dense, cache local BM25 |

## Sécurité du code modifié

- Chaque fichier modifié par l'agent est sauvegardé avant écriture : bouton ↶ sous le message pour tout restaurer.
- Diff coloré dans chaque carte d'outil ; option « Valider chaque modification » (paramètres).
- Après modification : `cargo check` / `tsc` / `go vet` / `py_compile` automatiques, erreurs renvoyées au modèle.
- Bouton 🔒/🔓 dans la zone de saisie : mode **Bypass** (toutes les autorisations acceptées ; les commandes destructrices restent bloquées).
- Les sources des dépendances (`~/.cargo/registry/src`) se lisent avec `read_file`/`outline`/`grep`, en lecture seule, sans shell ni autorisation.
- ✏ modifier un message, ⟳ régénérer (fichiers restaurés d'abord), ⇩ exporter en Markdown.
- Historique long résumé automatiquement au-delà du seuil de compaction (100k tokens par défaut).

## Confidentialité

- Données 100 % locales (`%APPDATA%/Omega` ou `~/.local/share/Omega`), fichiers en `0600` sous Linux.
- Clés API chiffrées par DPAPI (liées à votre compte Windows).
- Abonnements : jetons OAuth scellés dans `credentials/` (DPAPI), rafraîchis en série ; jeton Claude longue durée (`claude setup-token`) accepté sans échange réseau.
- Images : redimensionnées et stockées dans la discussion locale, jamais envoyées à un moteur qui ne les lit pas.
- Anti-SSRF : redirections et résolutions DNS vers des adresses privées refusées ; pages web marquées « non fiables ».
- User-Agent générique, aucun identifiant envoyé hors des appels API.
- Proxy/Tor (`socks5h://127.0.0.1:9050`), détection VPN, kill-switch optionnel.
- Les clés API ne sont jamais transmises aux processus lancés par l'agent.

## Architecture

```
src/
  domain/   modèle pur : discussion, route, skills, outils, réglages
  infra/    Kimi / DeepSeek (SSE), Jev, disque, HTTP, processus, VPN, DPAPI
  app/      cas d'usage : routeur Jev, assemblage du contexte, boucle agent, auto-tests
  tools/    outils natifs (fs, code, graphe, grep, shell, sécurité, web, compression)
  ui/       interface egui (thème, sidebar, explorateur/onglets, chat, composer, planificateur, panneau outils, réglages)
```
