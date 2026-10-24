# Omega — agent IA local, anonyme, 100 % Rust

**Jev** (System One, TypeSafe) prend toutes les décisions de contrôle : skill, budget de raisonnement, web, risque, impact.
**DeepSeek V4.1 Flash** ou **Claude** écrit le code et exécute les outils (System Two). La zone de saisie affiche trois sélecteurs — fournisseur, **modèle exact** (liste du compte + identifiant libre) et effort — et le modèle réellement utilisé reste visible dans l'en-tête, chaque réponse et le panneau Outils.
Tout tient dans un seul binaire (Windows et Linux), sans serveur externe ni télémétrie.

## Démarrage

1. Windows : double-cliquer `Omega-0.4.1-x64.msi` (installation par utilisateur, sans droits admin, raccourci Bureau). Linux : `target/release/omega`.
2. Au premier lancement, la fenêtre ⚙ Paramètres s'ouvre. Coller la clé **DeepSeek** (platform.deepseek.com) et la clé **Jev** (console.typesafe.ai ; sans elle, un routeur local prend le relais). Claude passe par l'abonnement (bouton « Se connecter ») ou une clé API.
3. Choisir un dossier de travail (📁 en bas à gauche). Par défaut : `Documents\Omega` (`~/Documents/Omega` sous Linux).

Les clés peuvent aussi venir des variables `DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY` et `TYPESAFE_API_KEY`.

Règles permanentes d'un projet : `OMEGA.md` à sa racine (bouton « Créer OMEGA.md » dans le panneau Outils). `AGENTS.md` et `CLAUDE.md` sont aussi lus.

## Interface

- **Fournisseur · modèle · effort** (zone de saisie) : ⟳ récupère la liste réelle des modèles du compte, « ✎ Autre identifiant… » accepte n'importe quel id (ex. `claude-opus-5-5`). Le choix est mémorisé par fournisseur dans `settings.toml` et chaque réponse porte le modèle qui l'a produite. Une erreur du fournisseur (modèle absent du compte, 404) s'affiche telle quelle : aucun repli silencieux.
- **Fichiers** (bouton 📁 en haut) : arborescence du dossier de travail, clic pour ouvrir un fichier dans un onglet au-dessus de la discussion, visionneuse numérotée en lecture seule (512 Ko max, binaire détecté).
- **Images** : glisser-déposer, `Ctrl+V` (capture d'écran) ou 🖼 dans la zone de saisie. Redimensionnées à 1568 px, 4 max par message, vignettes dans la bulle. Envoyées uniquement aux moteurs qui lisent les images (Claude) ; DeepSeek les garde locales.
- **⏰ Programmer** : prompt + jour + heure + moteur + modèle + effort, répétition quotidienne optionnelle. À l'heure dite, Omega ouvre une discussion et l'exécute **toujours en bypass**, avec le modèle choisi au moment de la programmation, sans toucher à vos réglages, à votre brouillon ni à la discussion ouverte. Une tâche quotidienne garde son heure locale au changement d'heure. Les tâches sont enregistrées dans `%APPDATA%/Omega/schedules.json` et ne partent que si Omega est ouvert.
- **⚙ Paramètres → Économies de tokens · preuve** : chaque requête est mesurée avec les chiffres du fournisseur (entrée, cache lu/écrit, sortie, Jev) ; les leviers (lazy tools, RTK, playbooks, compaction, cache) sont mesurés en octets sur la vraie requête et convertis avec le ratio tokens/octet calibré sur vos requêtes, puis pondérés au tarif réel. **Banc d'essai** : Jev noté sur 7 demandes étiquetées, lazy tools / RTK / TOON en tokens exacts (`count_tokens` Claude), cache vérifié par deux requêtes réelles, style A/B, CLI-Anything.
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
| lazy-tool | Jev choisit le skill ; seuls ses outils sont envoyés (ensemble monotone par discussion → cache stable). Claude 5+ : `tools` constant (outils différés `defer_loading`) et ajouts par `tool_addition` en milieu de conversation → le cache n'est jamais invalidé ; repli automatique en mode classique si l'API refuse |
| Serena | `outline`, `find_symbol`, `replace_symbol` via tree-sitter (11 langages), édition vérifiée syntaxiquement ; `read_file` d'un fichier inchangé déjà lu renvoie un talon au lieu du contenu |
| code-review-graph | `blast_radius` : graphe de références petgraph, BFS inverse profondeur 3 |
| RTK | compression des sorties shell (ANSI, bruit, tests passés, doublons, tête/queue) + filtres par commande : cargo, git, pytest, npm |
| TOON | résultats tabulaires séparés par tabulations ; `grep` groupé par fichier, parallélisé |
| caveman | 4 styles de sortie (Normal → Ultra-caveman) |
| Délégation (Jev) | Jev estime l'effort (jamais au-dessus de votre choix, seulement si le cache est préservé) ; questions simples sans outil et résumés de compaction confiés à DeepSeek Flash (interrupteurs dans ⚙ Paramètres) |
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
- Commande arrêtée (délai dépassé ou ■ Stop) : tout l'arbre de processus est tué, aucun serveur ni watcher ne reste orphelin.
- Les sources des dépendances (`~/.cargo/registry/src`) se lisent avec `read_file`/`outline`/`grep`, en lecture seule, sans shell ni autorisation.
- ✏ modifier un message, ⟳ régénérer (fichiers restaurés d'abord), ⇩ exporter en Markdown.
- Historique append-only (cache préfixe et raisonnement signé Claude préservés), résumé automatiquement au-delà du seuil de compaction (100k tokens par défaut).

## Confidentialité

- Données 100 % locales (`%APPDATA%/Omega` ou `~/.local/share/Omega`), fichiers en `0600` sous Linux.
- Clés API chiffrées par DPAPI (liées à votre compte Windows).
- Abonnements : jetons OAuth scellés dans `credentials/` (DPAPI), rafraîchis en série ; jeton Claude longue durée (`claude setup-token`) accepté sans échange réseau.
- Images : redimensionnées et stockées dans la discussion locale, jamais envoyées à un moteur qui ne les lit pas.
- Anti-SSRF : redirections et résolutions DNS vers des adresses privées refusées ; pages web marquées « non fiables ».
- User-Agent générique, aucun identifiant envoyé hors des appels API.
- Proxy/Tor (`socks5h://127.0.0.1:9050`), détection VPN locale (adaptateurs), kill-switch optionnel ; la vérification de l'IP de sortie (ipwho.is) est **désactivée par défaut** et s'active dans ⚙ Paramètres.
- Les clés API ne sont jamais transmises aux processus lancés par l'agent (`MOONSHOT_API_KEY`, `DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `TYPESAFE_API_KEY` sont retirées de leur environnement) ; un échec de chiffrement DPAPI refuse l'écriture plutôt que de stocker une clé en clair.

## Architecture

```
src/
  domain/   modèle pur : discussion, route, skills, catalogue de modèles, outils, réglages
  infra/    clients des moteurs (SSE), Jev, disque, HTTP, processus, VPN, DPAPI
  app/      cas d'usage : routeur Jev, assemblage du contexte, boucle agent, auto-tests
  tools/    outils natifs (fs, code, graphe, grep, shell, sécurité, web, compression)
  ui/       interface egui (thème, sidebar, explorateur/onglets, chat, composer, planificateur, panneau outils, réglages)
```
