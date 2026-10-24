//! End-to-end check with the saved settings and real providers: each prompt runs a full turn in a throwaway
//! project and reports routing, the engine/model of every answer, tokens, cache and errors.
//! `cargo test -- --ignored e2e_live --nocapture` (uses the subscriptions and keys configured in Omega).

use std::{
    sync::{Arc, mpsc},
    time::Instant,
};

use super::{Emitter, Env, Event, agent};
use crate::{
    domain::{
        chat::{Conversation, Message, Role, Usage},
        engine::Engine,
    },
    infra::store,
    tools::webindex::WebIndex,
};

const PROMPTS: &[(&str, &str)] = &[
    ("simple → Gemini Flash", "Bonjour ! En une phrase, qu'est-ce que Rust ?"),
    ("texte → Haiku", "Dans hello.py, remplace le texte affiché par 'Bonjour Omega'."),
    ("tests → délégués", "Écris des tests unitaires pytest pour calc.py dans test_calc.py."),
    ("moyen → Sonnet", "Ajoute à calc.py une fonction divide(a, b) avec docstring qui lève ValueError si b vaut 0."),
    ("sécurité → Opus", "parse_expr dans calc.py utilise eval : explique le risque de sécurité puis corrige-le de façon sûre."),
];

fn project() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("omega-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hello.py"), "print('Hello')\n").unwrap();
    std::fs::write(
        dir.join("calc.py"),
        "def add(a, b):\n    return a + b\n\n\ndef parse_expr(text):\n    return eval(text)\n",
    )
    .unwrap();
    dir
}

/// Several engines in one conversation: signed reasoning, tool ids and caches must survive every switch.
const CHAIN: &[(&str, &str)] = &[
    ("refactor → Sonnet", "Refactorise calc.py : regroupe les fonctions dans une classe Calculator qui garde l'historique des opérations."),
    ("simple → Gemini", "Merci ! C'est quoi la différence entre une liste et un tuple en Python, en deux phrases ?"),
    ("texte → Haiku", "Dans hello.py, ajoute une deuxième ligne qui affiche 'Au revoir'."),
    ("difficile → Opus", "Analyse la sécurité de toute la classe Calculator et corrige ce qui est dangereux."),
];

#[tokio::test]
#[ignore = "réseau + abonnements réels"]
async fn e2e_chain() {
    let (mut settings, _) = store::load_settings();
    let dir = project();
    settings.workspace = Some(dir.clone());
    settings.engine = Engine::Claude;
    settings.bypass = true;
    let web = Arc::new(WebIndex::open(&std::env::temp_dir().join("omega-e2e-web")));
    let env = Arc::new(Env::new(settings, web).unwrap());
    let mut conv = Conversation::default();
    for (label, prompt) in CHAIN {
        conv.push(Message::user((*prompt).to_owned()));
        let (tx, rx) = mpsc::channel();
        let start = Instant::now();
        agent::run(conv.clone(), env.clone(), Emitter::new(tx, Arc::new(|| {}))).await;
        println!("\n=== {label} ({:.1} s) — {prompt}", start.elapsed().as_secs_f32());
        let mut usage = Usage::default();
        for event in rx.try_iter() {
            match event {
                Event::Routed { route, toolset, thinking_from, prefix, preamble, tools_added, .. } => {
                    println!(
                        "  route: skill={} difficulté={:.2} simple={:.2} routine={:.2} outils={} ajoutés={tools_added:?}",
                        route.skill.id(),
                        route.difficulty(),
                        route.simple,
                        route.routine,
                        toolset.len()
                    );
                    conv.toolset = toolset;
                    conv.thinking_from = thinking_from;
                    conv.prefix = prefix;
                    if let Some(m) = conv.last_user_mut() {
                        m.route = Some(route);
                        m.preamble = preamble;
                        m.tools_added = tools_added;
                    }
                }
                Event::Message { msg, .. } => {
                    if msg.role == Role::Assistant {
                        let calls: Vec<String> = msg.tool_calls.iter().map(|c| c.name.clone()).collect();
                        let text: String = msg.content.chars().take(120).collect();
                        println!("  [{}] calls={calls:?} {}", msg.model, text.replace('\n', " "));
                    }
                    conv.push(msg);
                }
                Event::Usage { usage: u, .. } => usage += u,
                Event::Measured { sample, thinking_from, .. } => {
                    conv.savings.record(&sample);
                    conv.thinking_from = thinking_from;
                }
                Event::Context { tokens, .. } => conv.context = tokens,
                Event::Compacted { summary, .. } => conv.summary = Some(summary),
                Event::Cleared { indices, .. } => super::clear::apply(&mut conv, &indices),
                Event::Done { error: Some(e), .. } => println!("  ERREUR: {e}"),
                _ => {}
            }
        }
        println!(
            "  tokens: entrée {} (cache lu {}, écrit {}) · sortie {} · cache {:.0} %",
            usage.prompt,
            usage.cached,
            usage.written,
            usage.completion,
            usage.cache_ratio() * 100.0
        );
    }
}

#[tokio::test]
#[ignore = "réseau + abonnements réels"]
async fn e2e_live() {
    let (mut settings, _) = store::load_settings();
    let dir = project();
    settings.workspace = Some(dir.clone());
    settings.engine = Engine::Claude;
    settings.bypass = true;
    let web = Arc::new(WebIndex::open(&std::env::temp_dir().join("omega-e2e-web")));
    let env = Arc::new(Env::new(settings, web).unwrap());
    println!(
        "main={} {} · helpers={:?}",
        env.llm.engine.label(),
        env.llm.model,
        env.helpers.iter().map(|h| format!("{}:{}", h.engine.label(), h.model)).collect::<Vec<_>>()
    );
    for (label, prompt) in PROMPTS {
        let mut conv = Conversation::default();
        conv.push(Message::user((*prompt).to_owned()));
        let (tx, rx) = mpsc::channel();
        let emit = Emitter::new(tx, Arc::new(|| {}));
        let start = Instant::now();
        agent::run(conv, env.clone(), emit).await;
        println!("\n=== {label} ({:.1} s) — {prompt}", start.elapsed().as_secs_f32());
        let mut usage = Usage::default();
        for event in rx.try_iter() {
            match event {
                Event::Routed { route, tools_added, toolset, .. } => println!(
                    "  route: {:?} skill={} effort={:?} difficulté={:.2} simple={:.2} routine={:.2} code={:.2} risque={:.2} playbook={:?} outils={} ajoutés={:?}",
                    route.source,
                    route.skill.id(),
                    route.effort,
                    route.difficulty(),
                    route.simple,
                    route.routine,
                    route.code,
                    route.risk,
                    route.playbook,
                    toolset.len(),
                    tools_added
                ),
                Event::Message { msg, .. } if msg.role == Role::Assistant => {
                    let calls: Vec<String> = msg.tool_calls.iter().map(|c| c.name.clone()).collect();
                    let text: String = msg.content.chars().take(160).collect();
                    println!(
                        "  [{} · {}] calls={calls:?} {}",
                        msg.engine.map_or("?", |e| e.label()),
                        msg.model,
                        text.replace('\n', " ")
                    );
                }
                Event::Usage { usage: u, .. } => usage += u,
                Event::Done { error: Some(e), .. } => println!("  ERREUR: {e}"),
                _ => {}
            }
        }
        println!(
            "  tokens: entrée {} (cache lu {}, écrit {}) · sortie {} · jev {} · cache {:.0} %",
            usage.prompt,
            usage.cached,
            usage.written,
            usage.completion,
            usage.jev,
            usage.cache_ratio() * 100.0
        );
    }
    for f in ["hello.py", "calc.py", "test_calc.py"] {
        println!("\n--- {f}\n{}", std::fs::read_to_string(dir.join(f)).unwrap_or_else(|_| "(absent)".into()));
    }
}
