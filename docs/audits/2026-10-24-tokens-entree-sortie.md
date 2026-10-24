# Tokens d'entrée et de sortie — Omega 0.7.0

Date : 2026-10-24 · Moteurs : Claude `claude-opus-5-5` (abonnement), DeepSeek `deepseek-flash` (clé), Jev.

## Où partent vraiment les tokens (vos 6 conversations locales, agrégats seuls)

Lecture seule de `%APPDATA%\Omega\conversations` (scripts dans le ledger SDD, aucun contenu affiché).

| Mesure | Valeur |
|---|---|
| Requêtes mesurées | 281 |
| Entrée | 48,0 M tokens, dont 46,8 M lus du cache (97,6 %) |
| Contexte moyen par requête | **171 k tokens** |
| Sortie | 270 k tokens |
| Étapes / tour | 856 étapes pour 27 tours (≈ 32) ; 470 étapes à appels multiples |

| Sortie (caractères) | Valeur |
|---|---|
| Narration avant les appels | 12 k (négligeable) |
| Réponses finales | 37 k |
| Arguments d'outils | **929 k** : `edit_file` old 168 k (dont 101 k dans 106 blocs > 400 car.), new 266 k ; `write_file` 282 k |
| Raisonnement | 748 k |

| Entrée : résultats d'outils (caractères) | Valeur |
|---|---|
| `read_file` | 1,40 M (342 appels, 4,1 k en moyenne) |
| `shell` / `web_fetch` / `grep` | 200 k / 152 k / 131 k |

Conclusion : le poste dominant est la **relecture du contexte à chaque étape**. La compaction n'agit
qu'entre les tours, alors qu'un tour long fait grossir le contexte sans limite.

## Leviers ajoutés

| Levier | Moteurs | Effet mesuré (API réelle) |
|---|---|---|
| Effacement des anciens résultats d'outils en cours de tour (`app/clear.rs`) | tous | Au-delà de `compact_at`, les plus anciens résultats volumineux (hors 6 derniers) sont remplacés par un talon d'une ligne, jusqu'à 60 % du budget, par lots ≥ 15 % du budget (une seule rupture de cache par lot). Banc : requête 20,2 → 11,5 Ko ; Claude 5.5 accepte l'historique effacé, raisonnement signé compris ; relecture renvoyée en entier (talon `read_file` désactivé pour un résultat effacé). L'UI garde le texte. |
| Discipline de sortie (`prompts::OUTPUT`) | tous | Pas de narration entre appels, appels indépendants groupés, pas de reformulation, de récapitulatif ni d'écho. A/B boucle d'outils : texte visible −4 %, −31 %, −41 % (3 runs, 1 échantillon chacun) |
| `edit_file` + `until` | tous | Long bloc : `old` = premières lignes, `until` = dernières ; plafond 300 lignes, CRLF respecté |
| DeepSeek `Off` → `thinking: disabled` | DeepSeek | Avant : réflexion facturée à l'effort `high` par défaut. Après : 0 token de réflexion (historique outils + `reasoning_content` accepté) |
| Résumés de compaction à `Effort::Off` | tous | Pas de réflexion pour résumer |
| Délégué DeepSeek à l'effort estimé par Jev | DeepSeek | Effort plafonné comme le moteur principal |
| Normes de code limitées aux langages du projet | tous | −85 tokens par requête ; compensé par les nouvelles règles de sortie : préfixe 3680 → 3679 tok, lu à 100 % du cache |

## Écartés, avec raison

- **Retirer `reasoning_content` de l'historique DeepSeek** : obligatoire avec `tools`, sinon 400 (doc DeepSeek).
- **Retirer les diagrammes des playbooks Superpowers** : gain de 0 à 18 % par playbook, au prix de la copie verbatim vérifiée.
- **Plafond `max_tokens` plus bas** : la facture porte sur la sortie réelle ; un plafond ne fait que tronquer.
