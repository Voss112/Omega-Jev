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

/// What routing needs from the conversation, copied out so the turn can compact the history meanwhile.
pub struct Input {
    pub query: String,
    previous: Option<String>,
    turns: usize,
    last_route: Option<Route>,
}

impl Input {
    pub fn of(conv: &Conversation) -> Self {
        Self {
            query: conv.messages.last().map(|m| m.content.clone()).unwrap_or_default(),
            previous: conv.messages.iter().rev().filter(|m| m.role == Role::User).nth(1).map(|m| clip(&m.content, 300)),
            turns: conv.user_turns(),
            last_route: conv.messages.iter().rev().skip(1).find_map(|m| m.route.clone()),
        }
    }

    /// The previous verdict, when the query is a bare "continue".
    fn follow_up(&self) -> Option<&Route> {
        let q = self.query.trim().trim_end_matches(['.', '!', ' ']).to_lowercase();
        CONTINUATIONS.contains(&q.as_str()).then_some(self.last_route.as_ref()).flatten()
    }
}

/// Returns the route and the Jev tokens spent. A bare follow-up ("continue") keeps the previous verdict (no Jev call).
pub async fn route(env: &Env, input: &Input) -> (Route, u64) {
    if let Some(previous) = input.follow_up() {
        return (previous.clone(), 0);
    }
    if let Some(jev) = &env.jev
        && let Ok(found) = ask_jev(jev, input).await
    {
        return found;
    }
    (local(&input.query), 0)
}

pub(super) async fn ask_jev(jev: &Jev, input: &Input) -> Result<(Route, u64)> {
    let state = json!({ "request": clip(&input.query, 1500), "previous_request": input.previous, "turn": input.turns });
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
        (
            "effort".to_owned(),
            Question::Score {
                instructions: "How much reasoning does answering this request well take?".into(),
                levels: ["trivial", "light", "standard", "deep"].map(String::from).to_vec(),
            },
        ),
        (
            "simple".to_owned(),
            Question::noul("Simple chat or factual question answerable without tools, files or deep reasoning?"),
        ),
        (
            "routine".to_owned(),
            Question::noul(
                "Is the request ONLY routine work a fast model does as well: writing or running tests (unit, integration, end-to-end), adding comments or docstrings to existing code without changing its behavior, or summarizing? A new feature, function or behavior change is not routine.",
            ),
        ),
        (
            "design".to_owned(),
            Question::noul(
                "Visual or UI design work: layouts, components, styling, animations, shaders, landing pages, design systems?",
            ),
        ),
    ];
    let a = jev.ask(state, &questions).await?;
    let (skill, confidence) = a.choice("skill").and_then(|(id, c)| Some((Skill::parse(&id)?, c))).context("skill")?;
    let mut route = Route {
        skill,
        source: Source::Jev,
        confidence,
        web: a.noul("web").unwrap_or(0.0),
        code: a.noul("code").unwrap_or(0.0),
        risk: a.noul("risk").unwrap_or(0.0),
        blast: a.noul("blast").unwrap_or(0.0),
        apps: a.noul("apps").unwrap_or(0.0),
        playbook: a.choice("playbook").map(|(id, _)| id).filter(|id| Playbook::find(id).is_some()),
        effort: a.score("effort"),
        simple: a.noul("simple").unwrap_or(0.0),
        routine: a.noul("routine").unwrap_or(0.0),
        design: a.noul("design").unwrap_or(0.0),
    };
    route.playbook = route.playbook.take().filter(|_| needs_method(&route));
    Ok((route, a.tokens))
}

/// Below this effort a request is a one-liner: a methodology playbook would cost more tokens and steps than the task.
const TRIVIAL: f32 = 0.25;

fn needs_method(route: &Route) -> bool {
    route.risky() || route.effort.is_none_or(|e| e >= TRIVIAL)
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

/// Routine work (local fallback for the Jev `routine` question).
const ROUTINE_WORDS: &[&str] = &[
    "test unitaire",
    "tests unitaires",
    "unit test",
    "e2e",
    "end-to-end",
    "end to end",
    "test d'intégration",
    "tests d'intégration",
    "integration test",
    "commente",
    "commentaire",
    "docstring",
    "résume",
    "resume",
    "summarize",
];

/// Visual design work (local fallback for the Jev `design` question).
const DESIGN_WORDS: &[&str] = &[
    "design",
    "ui ",
    "interface",
    "landing",
    "css",
    "tailwind",
    "animation",
    "shader",
    "composant visuel",
    "maquette",
    "mockup",
    "curseur",
    "cursor effect",
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
        effort: None,
        simple: 0.0,
        routine: flag(ROUTINE_WORDS.iter().any(|w| q.contains(w))),
        design: flag(DESIGN_WORDS.iter().any(|w| q.contains(w))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::chat::Message;

    fn conversation(last: &str) -> Conversation {
        let mut conv = Conversation::default();
        conv.push(Message { route: Some(local("audit de sécurité du module auth")), ..Message::user("audit auth".into()) });
        conv.push(Message::assistant("ok".into(), String::new(), Vec::new()));
        conv.push(Message::user(last.into()));
        conv
    }

    #[test]
    fn a_bare_follow_up_reuses_the_previous_route() {
        let input = Input::of(&conversation("Continue."));
        assert_eq!(input.query, "Continue.");
        assert_eq!(input.follow_up().map(|r| r.skill), Some(Skill::Security));
    }

    #[test]
    fn trivial_turns_skip_the_methodology_playbook() {
        let trivial = Route { effort: Some(0.1), ..local("ajoute une fonction") };
        assert!(!needs_method(&trivial));
        assert!(needs_method(&Route { effort: Some(0.6), ..trivial.clone() }));
        assert!(needs_method(&Route { risk: 0.9, ..trivial.clone() }), "sensible : méthode gardée");
        assert!(needs_method(&Route { effort: None, ..trivial }), "sans estimation : gardée");
    }

    #[test]
    fn local_routing_spots_routine_and_design_work() {
        assert!(local("écris les tests unitaires du parser").routine_task());
        assert!(local("ajoute une animation shader sur la landing page").needs_design());
        assert!(!local("corrige le bug d'auth").routine_task());
    }

    #[test]
    fn a_new_request_is_routed_again() {
        let input = Input::of(&conversation("explique ce code"));
        assert!(input.follow_up().is_none());
        assert_eq!(input.previous.as_deref(), Some("audit auth"));
        assert_eq!(input.turns, 2);
    }
}
