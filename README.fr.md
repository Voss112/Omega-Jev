# Omega — agent IA local, anonyme, 100 % Rust

**Jev** (System One, TypeSafe) prend toutes les décisions de contrôle : skill, budget de raisonnement, web, risque, impact.
**DeepSeek V4.1 Flash**, **Claude** ou **Gemini** écrit le code et exécute les outils (System Two). La zone de saisie affiche trois sélecteurs — fournisseur, **modèle exact** (liste du compte + identifiant libre) et effort — et le modèle réellement utilisé reste visible dans l'en-tête, chaque réponse et le panneau Outils.
Tout tient dans un seul binaire (Windows et Linux), sans serveur externe ni télémétrie.

## Démarrage

1. Windows : double-cliquer `Omega-0.9.0-x64.msi` (installation par utilisateur, sans droits admin, raccourci Bureau). Linux : `target/release/omega`.
2. Au premier lancement, la fenêtre ⚙ Paramètres s'ouvre. Coller la clé **DeepSeek** (platform.deepseek.com) et la clé **Jev** (console.typesafe.ai ; sans elle, un routeur local prend le relais). Claude et Gemini passent par l'abonnement (bouton « Se connecter » : Claude Pro/Max, Google AI Pro/Ultra via Gemini Code Assist) ou une clé API. GitHub se connecte au même endroit (code à saisir sur github.com).
3. Choisir un dossier de travail (📁 en bas à gauche). Par défaut : `Documents\Omega` (`~/Documents/Omega` sous Linux).

Les clés peuvent aussi venir des variables `DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY` et `TYPESAFE_API_KEY`.

Règles permanentes d'un projet : `OMEGA.md` à sa racine (bouton « Créer OMEGA.md » dans le panneau Outils). `AGENTS.md` est aussi lu. `CLAUDE.md` n'est **jamais** envoyé (écrit pour Claude Code, souvent long : il serait refacturé à chaque requête).

## Interface

- **Fournisseur · modèle · effort** (zone de saisie) : ⟳ récupère la liste réelle des modèles du compte, « ✎ Autre identifiant… » accepte n'importe quel id (ex. `claude-opus-5-5`). Le choix est mémorisé par fournisseur dans `settings.toml` et chaque réponse porte le modèle qui l'a produite. Une erreur du fournisseur (modèle absent du compte, 404) s'affiche telle quelle : aucun repli silencieux.
- **Fichiers** (bouton 📁 en haut) : arborescence du dossier de travail, clic pour ouvrir un fichier dans un onglet au-dessus de la discussion. Coloration syntaxique (Rust, Python, JS/TS, Go, C/C++, Java, C#, JSON, YAML, Markdown…), numéros de ligne, **✎ Modifier** puis `Ctrl+S` pour enregistrer (fins de ligne CRLF/LF respectées). Un onglet non enregistré ne se ferme pas et n'est jamais écrasé si l'agent modifie le fichier (« modifié sur le disque » + Recharger). Lecture seule au-delà de 512 Ko et pour les binaires.
- **Tâches** (bouton en haut ou `/tasks`) : toutes les tâches programmées — à venir, en pause, terminées — avec compte à rebours, historique des exécutions et lien vers chaque discussion ouverte ; Lancer maintenant, Pause/Reprendre, Modifier, Supprimer.
- **`/clear`** : vide le contexte (nouvelle discussion, l'ancienne reste dans l'historique).
- **Images** : glisser-déposer, `Ctrl+V` (capture d'écran) ou 🖼 dans la zone de saisie. Redimensionnées à 1568 px, 4 max par message, vignettes dans la bulle. Envoyées uniquement aux moteurs qui lisent les images (Claude) ; DeepSeek les garde locales.
- **🎙 Dictée** (`Ctrl+M` ou bouton micro ; un à-coup du pilote audio — « buffer underrun or overrun » — n'interrompt plus la dictée) : parlez, chaque phrase s'écrit dans la zone de saisie dès la pause qui la termine ; `Entrée` arrête et envoie, `Échap` arrête. Whisper large-v3-turbo **en local sur le GPU** (Vulkan ; CPU sans GPU) : 0,2 s pour une phrase, 0,3 s pour 11 s de parole sur RX 6950 XT. Le silence ne produit rien. Langue dans ⚙ Paramètres (français par défaut, détection automatique possible). Au premier usage, le modèle (574 Mo) est téléchargé une fois depuis Hugging Face, via le proxy configuré, et son SHA-256 est vérifié.
- **Tableaux** : colonnes dimensionnées sur la largeur de lecture, texte replié dans chaque cellule.
- **⏰ Programmer** : prompt + jour + heure + moteur + modèle + effort, répétition quotidienne optionnelle. À l'heure dite, Omega ouvre une discussion et l'exécute **toujours en bypass**, avec le modèle choisi au moment de la programmation, sans toucher à vos réglages, à votre brouillon ni à la discussion ouverte. Une tâche quotidienne garde son heure locale au changement d'heure. Les tâches sont enregistrées dans `%APPDATA%/Omega/schedules.json` et ne partent que si Omega est ouvert.
- **⚙ Paramètres → Économies de tokens · preuve** : chaque requête est mesurée avec les chiffres du fournisseur (entrée, cache lu/écrit, sortie, Jev) ; les leviers (lazy tools, RTK, playbooks, compaction, cache) sont mesurés en octets sur la vraie requête et convertis avec le ratio tokens/octet calibré sur vos requêtes, puis pondérés au tarif réel. **Banc d'essai** : Jev noté sur 7 demandes étiquetées, lazy tools / RTK / TOON en tokens exacts (`count_tokens` Claude), cache vérifié par deux requêtes réelles, style A/B, CLI-Anything.
- **Dernier verdict Jev** : skill, source, confiance, cinq métriques (code, web, risque, impact, apps) et outils chargés, en cartes compactes.
- **Étapes max / tour** : case **Infinie** pour les longues tâches sans surveillance (■ Stop reste possible).

## Délégation, paliers Claude, tests en continu (⚙ Paramètres → Comportement)

- **Délégation** : questions simples, résumés, **tests unitaires / intégration / end-to-end** et **commentaires** partent sur **Gemini 3.8 Flash** (modèle réglable : « Gemini Flash »), puis **DeepSeek V4.1 Flash** si Gemini est indisponible ou à 100 % de quota (429 *quota* : Gemini est mis de côté 30 min, sans réessai inutile), puis le moteur principal. Jev décide (questions `simple` et `routine`) ; rien de sensible (sécurité, API publique, apps) n'est délégué. Interrupteurs : *Délégation*, *Gemini Flash d'abord*, *Tests & commentaires*.
- **Modèle auto (Claude)** : Jev note la difficulté de chaque tour (effort estimé, skill, risque, impact) : texte ou retouche simple → **Haiku** en effort Low, travail courant → **Sonnet**, difficile → votre modèle. Jamais au-dessus de votre choix, même génération (opus-5-5 → sonnet-5-5 → haiku-5-5). Pas de bascule quand l'historique dépasse 40 k tokens et que le cache du modèle moins cher est froid. Si le palier inférieur échoue ou dépasse 24 étapes, votre modèle reprend le tour en cours. Chaque réponse affiche le modèle qui l'a écrite.
- **Tests en continu** : les tests tournent en arrière-plan pendant que l'agent code. Watcher natif s'il est installé — Rust `cargo watch`, JS/TS `vitest` / `jest --watchAll`, Python `ptw`, Go `gotestsum --watch`, .NET `dotnet watch test`, Java `gradle --continuous` — sinon le watcher Omega (surveillance du dossier + relance immédiate à chaque modification de l'agent) avec `cargo test`, `npm test`, `pytest`, `go test`, `mvn test`, `ctest`, `rspec`, `phpunit`, `deno test`. Le résultat arrive dans le tour sous forme de note `[live-tests]` ; l'agent ne relance plus la suite (une commande de test renvoie le résultat du watcher), et la fin du tour signale un échec du dernier changement. État dans le panneau Outils ; aucun processus ne survit à la fermeture.
- **Kits** (outil `kits`, chargé par Jev pour le design et le web, utilisé seulement si l'agent le juge utile) : canvas-ui, paper-shaders, motion-primitives, ui-layouts, cursify, design-taste, better-design, superdesign, scrapling, agent-reach. Installation par commandes figées, approuvées comme toute commande shell.
- **Outils d'applications inactifs** (CLI-Anything type Blender, kits, audit) : retirés automatiquement de la requête après 3 tours sans appel.
- **GitHub** : connexion par code (device flow) ou jeton collé ; `git push` et `gh` du shell de l'agent s'authentifient avec (variables d'environnement du seul processus, rien d'écrit dans `.gitconfig`, jeton masqué dans les sorties). Interrupteur *Shell de l'agent*.
- **Shell plus rapide** : une commande simple (`cargo test`, `git status`, `npm run build`…) démarre directement sans PowerShell (~25 ms au lieu de ~170 ms), et l'avis de Jev sur une commande inconnue est mémorisé pour la session.

## Compiler

```sh
cargo build --release                      # binaire optimisé (LTO), icône intégrée
cargo test                                 # tests outils, domaine, DPAPI, SSRF, checkpoints
cargo test --release -- --ignored voice_live dictation_live --nocapture   # dictée réelle (modèle + GPU)
powershell -File installer/build-msi.ps1   # MSI (WiX 3.x dans %USERPROFILE%\tools\wix314 ou $env:WIX_BIN)
```

Windows (cible `x86_64-pc-windows-gnu`) : la dictée compile whisper.cpp. Prérequis, tous chemins dans `.cargo/config.toml` : MinGW-w64 (gcc, CMake, Ninja), `libclang.dll` pour bindgen, et en-têtes Vulkan + `glslc` + `libvulkan-1.dll.a` (paquets MSYS2 ucrt64). L'exe n'importe que des DLL système et `vulkan-1.dll` (fourni par le pilote GPU) ; la C++ runtime est liée statiquement (`build.rs`). `vendor/whisper-rs` : copie de whisper-rs 0.16.0 avec un correctif MinGW (voir `vendor/README.md`).

Linux : paquets X11/Wayland usuels requis à l'exécution (comme toute app egui).

## Ce qui remplace quoi (in-process)

| Outil d'origine | Intégration Omega |
|---|---|
| lazy-tool | Jev choisit le skill ; seuls ses outils sont envoyés (ensemble monotone par discussion → cache stable). Claude 5+ : `tools` constant (outils différés `defer_loading`) et ajouts par `tool_addition` en milieu de conversation → le cache n'est jamais invalidé ; repli automatique en mode classique si l'API refuse |
| Serena | `outline`, `find_symbol`, `replace_symbol` via tree-sitter (11 langages), édition vérifiée syntaxiquement ; `read_file` d'un fichier inchangé déjà lu renvoie un talon au lieu du contenu |
| code-review-graph | `blast_radius` : graphe de références petgraph, BFS inverse profondeur 3 |
| RTK | compression des sorties shell (ANSI, bruit, tests passés, doublons, tête/queue) + filtres par commande : cargo, git, pytest, npm |
| TOON | résultats tabulaires séparés par tabulations ; `grep` groupé par fichier, parallélisé |
| caveman | 4 styles de sortie (Normal → Ultra-caveman) + discipline commune à tous les moteurs : pas de narration entre outils, appels groupés, pas d'écho ni de récapitulatif ; `edit_file` + `until` pour remplacer un long bloc sans le recopier |
| Micro-compaction (Claude Code) | au-delà du budget de contexte en plein tour, les anciens résultats d'outils volumineux sont remplacés par un talon d'une ligne (texte gardé dans l'interface) ; DeepSeek `Off` coupe vraiment la réflexion ; résumés sans réflexion |
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
- Voix : capturée et transcrite sur la machine ; aucun audio n'est enregistré sur disque ni envoyé. Le micro n'est ouvert que pendant la dictée.
- Anti-SSRF : redirections et résolutions DNS vers des adresses privées refusées ; pages web marquées « non fiables ».
- User-Agent générique, aucun identifiant envoyé hors des appels API.
- Proxy/Tor (`socks5h://127.0.0.1:9050`), détection VPN locale (adaptateurs), kill-switch optionnel ; la vérification de l'IP de sortie (ipwho.is) est **désactivée par défaut** et s'active dans ⚙ Paramètres.
- Les clés API ne sont jamais transmises aux processus lancés par l'agent (`MOONSHOT_API_KEY`, `DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `TYPESAFE_API_KEY` sont retirées de leur environnement) ; un échec de chiffrement DPAPI refuse l'écriture plutôt que de stocker une clé en clair.

## Architecture

```
src/
  domain/   modèle pur : discussion, route, skills, catalogue de modèles, outils, réglages
  infra/    clients des moteurs (SSE), Jev, disque, HTTP, processus, VPN, DPAPI, voix (micro, Whisper)
  app/      cas d'usage : routeur Jev, assemblage du contexte, boucle agent, dictée, auto-tests
  tools/    outils natifs (fs, code, graphe, grep, shell, sécurité, web, compression)
  ui/       interface egui (thème, sidebar, explorateur/onglets, chat, composer, planificateur, panneau outils, réglages)
```
