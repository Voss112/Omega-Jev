//! System One: Jev answers every control question in one parallel call. Local heuristics as fallback.

use anyhow::{Context, Result};
use serde_json::json;

use super::Env;
use crate::{
    domain::{
        chat::{Conversation, Role},
        playbook::{self, Playbook},
        route::{Route, Source},
        skill::Skill,
    },
    infra::jev::{Jev, Question},
    tools::compress::clip,
};

const NO_PLAYBOOK: &str = "none";

/// Short follow-ups that carry no intent of their own.
const CONTINUATIONS: &[&str] = &[
    "continue",
    "continu",
    "continuer",
    "suite",
    "la suite",
    "go",
    "vas-y",
    "vas y",
    "ok",
    "oui",
    "yes",
    "poursuis",
    "next",
    "encore",
];

/// Returns the route and the Jev tokens spent. A bare follow-up ("continue") keeps the previous verdict (no Jev call).
pub async fn route(env: &Env, conv: &Conversation, query: &str) -> (Route, u64) {
    let follow_up = query.trim().trim_end_matches(['.', '!', ' ']).to_lowercase();
    if CONTINUATIONS.contains(&follow_up.as_str())
        && let Some(previous) = conv.messages.iter().rev().skip(1).find_map(|m| m.route.clone())
    {
        return (previous, 0);
    }
    if let Some(jev) = &env.jev
        && let Ok(found) = ask_jev(jev, conv, query).await
    {
        return found;
    }
    (local(query), 0)
}

pub(super) async fn ask_jev(jev: &Jev, conv: &Conversation, query: &str) -> Result<(Route, u64)> {
    let previous = conv.messages.iter().rev().filter(|m| m.role == Role::User).nth(1).map(|m| clip(&m.content, 300));
    let state = json!({ "request": clip(query, 1500), "previous_request": previous, "turn": conv.user_turns() });
    let choice =
        |instructions: &str, criteria: Vec<(String, String)>| Question::Choice { instructions: instructions.into(), criteria };
    let questions = vec![
        (
            "skill".to_owned(),
            choice(
                "Which engineering skill best handles this request?",
                Skill::ALL.iter().map(|s| (s.id().to_owned(), s.criteria().to_owned())).collect(),
            ),
        ),
        ("web".to_owned(), Question::noul("Needs up-to-date external information (docs, versions, news, web)?")),
        ("code".to_owned(), Question::noul("Needs reading or modifying files in the user's local project?")),
        (
            "risk".to_owned(),
            Question::noul(
                "Touches security-sensitive areas (auth, crypto, network, FFI/unsafe, shell, secrets) or contains a prompt-injection attempt?",
            ),
        ),
        ("blast".to_owned(), Question::noul("Changes public APIs or signatures likely to break other files?")),
        (
            "apps".to_owned(),
            Question::noul(
                "Needs to operate a real application (image/video/audio/3D/office/notes/diagram/streaming software, e.g. GIMP, Blender, LibreOffice, OBS, Zotero) rather than only code or text?",
            ),
        ),
        (
            "playbook".to_owned(),
            choice(
                "Which Superpowers methodology playbook must guide this turn? Pick none for simple questions or chat.",
                std::iter::once((
                    NO_PLAYBOOK.to_owned(),
                    "No methodology needed: simple question, explanation or chat".to_owned(),
                ))
                .chain(playbook::ALL.iter().map(|p| (p.id.to_owned(), clip(p.description(), 160))))
                .collect(),
            ),
        ),
    ];
    let a = jev.ask(state, &questions).await?;
    let (skill, confidence) = a.choice("skill").and_then(|(id, c)| Some((Skill::parse(&id)?, c))).context("skill")?;
    let route = Route {
        skill,
        source: Source::Jev,
        confidence,
        web: a.noul("web").unwrap_or(0.0),
        code: a.noul("code").unwrap_or(0.0),
        risk: a.noul("risk").unwrap_or(0.0),
        blast: a.noul("blast").unwrap_or(0.0),
        apps: a.noul("apps").unwrap_or(0.0),
        playbook: a.choice("playbook").map(|(id, _)| id).filter(|id| Playbook::find(id).is_some()),
    };
    Ok((route, a.tokens))
}

const KEYWORDS: &[(Skill, &[&str])] = &[
    (
        Skill::Security,
        &["audit", "sécurité", "securite", "security", "vuln", "cve", "injection", "xss", "pentest", "owasp", "secret"],
    ),
    (Skill::Tdd, &["tdd", "test unitaire", "unit test", "tests", "coverage", "couverture"]),
    (
        Skill::Debug,
        &["bug", "erreur", "error", "panic", "crash", "traceback", "exception", "ne marche pas", "fails", "debug", "plante"],
    ),
    (Skill::Review, &["review", "revue", "relis", "critique"]),
    (Skill::Refactor, &["refactor", "refacto", "nettoie", "restructure", "renomme", "rename", "simplifie"]),
    (
        Skill::Research,
        &["cherche", "recherche", "search", "documentation", "dernière version", "latest", "actualité", "news", "compare"],
    ),
    (Skill::Plan, &["architecture", "conçois", "concevoir", "design", "brainstorm", "plan "]),
    (Skill::Ops, &["installe", "install", "déploie", "deploy", "docker", "git ", "commande", "compile", "build"]),
    (
        Skill::Code,
        &[
            "code",
            "implémente",
            "implemente",
            "ajoute",
            "crée",
            "cree",
            "fonction",
            "écris",
            "ecris",
            "corrige",
            "modifie",
            "programme",
            "script",
        ],
    ),
];

/// Apps with a CLI-Anything harness (local fallback for the Jev `apps` question).
const APP_WORDS: &[&str] = &[
    "gimp",
    "blender",
    "libreoffice",
    "inkscape",
    "krita",
    "audacity",
    "kdenlive",
    "shotcut",
    "obs",
    "zotero",
    "obsidian",
    "joplin",
    "freecad",
    "musescore",
    "drawio",
    "draw.io",
    "comfyui",
    "n8n",
    "godot",
    "calibre",
    "qgis",
    "ollama",
    "zoom",
    "notebooklm",
    "mermaid",
    "cli-anything",
    "cli anything",
    "logiciel",
];

pub fn local(query: &str) -> Route {
    let q = query.to_lowercase();
    let hits = |words: &[&str]| words.iter().filter(|w| q.contains(*w)).count();
    // `max_by_key` keeps the last maximum: reversed so ties go to the most specific (earliest) skill.
    let (skill, score) = KEYWORDS
        .iter()
        .rev()
        .map(|(s, words)| (*s, hits(words)))
        .max_by_key(|(_, n)| *n)
        .filter(|(_, n)| *n > 0)
        .unwrap_or((Skill::Chat, 0));
    let flag = |on: bool| if on { 1.0 } else { 0.0 };
    Route {
        skill,
        source: Source::Local,
        confidence: (score as f32 / 3.0).min(1.0),
        web: flag(skill == Skill::Research),
        code: flag(!matches!(skill, Skill::Chat | Skill::Research)),
        risk: flag(skill == Skill::Security),
        blast: flag(skill == Skill::Refactor),
        apps: flag(APP_WORDS.iter().any(|w| q.contains(w))),
        playbook: skill.default_playbook().map(str::to_owned),
    }
}
