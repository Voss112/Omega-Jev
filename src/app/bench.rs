//! Live token-economy benchmark: each lever measured with the provider's own token counts
//! (Claude `count_tokens`, real `usage` fields), Jev graded on labelled requests.

use std::{collections::BTreeSet, fmt::Write as _, time::Duration};

use anyhow::{Result, ensure};
use futures_util::future::join_all;
use serde_json::{Value, json};

use super::{Env, health::Health, prompts, router, savings};
use crate::{
    domain::{
        chat::{Conversation, Message, Usage},
        engine::{Effort, Wire},
        route::Route,
        savings::Rates,
        settings::Style,
        skill::Skill,
        tool::ToolKind,
    },
    infra::llm::{Completion, Llm},
    tools::{clihub, compress, schema},
};

const GROUP: &str = "Banc d'essai";

/// A labelled request: accepted skills (empty = any) and the flag Jev must raise.
struct Case {
    query: &'static str,
    skills: &'static [Skill],
    check: fn(&Route) -> bool,
}

fn quiet(r: &Route) -> bool {
    !r.needs_code() && !r.needs_web() && !r.needs_apps()
}

fn app_tools(r: &Route) -> bool {
    r.needs_apps() && r.tools().contains(&ToolKind::CliAnything)
}

const CASES: [Case; 7] = [
    Case {
        query: "Corrige le panic « index out of bounds » dans src/parser.rs ligne 42",
        skills: &[Skill::Debug, Skill::Code],
        check: Route::needs_code,
    },
    Case {
        query: "Écris des tests unitaires pour la fonction parse_date du module dates",
        skills: &[Skill::Tdd, Skill::Code],
        check: Route::needs_code,
    },
    Case {
        query: "Quelle est la dernière version stable de tokio et qu'apporte-t-elle ?",
        skills: &[Skill::Research],
        check: Route::needs_web,
    },
    Case {
        query: "Audite la sécurité du flux OAuth et du stockage des jetons de ce projet",
        skills: &[Skill::Security, Skill::Review],
        check: Route::risky,
    },
    Case { query: "Ouvre photo.png dans GIMP, recadre-la en 800x600 et exporte-la en JPEG", skills: &[], check: app_tools },
    Case { query: "Merci beaucoup pour ton aide, bonne soirée !", skills: &[Skill::Chat], check: quiet },
    Case {
        query: "Renomme la fonction publique load_cfg en load_config dans tout le projet",
        skills: &[Skill::Refactor, Skill::Code],
        check: Route::needs_code,
    },
];

fn graded(case: &Case, route: &Route) -> bool {
    (case.skills.is_empty() || case.skills.contains(&route.skill)) && (case.check)(route)
}

fn row(name: &'static str, result: Result<String>) -> Health {
    match result {
        Ok(detail) => Health { group: GROUP, name, ok: true, detail },
        Err(e) => Health { group: GROUP, name, ok: false, detail: format!("{e:#}") },
    }
}

pub async fn run(env: &Env) -> Vec<Health> {
    if !env.llm.configured() {
        return vec![row("Moteur", Err(anyhow::anyhow!("{} non configuré", env.llm.engine.label())))];
    }
    let llm = env.llm.with_effort(Effort::Low);
    let ((jev, jev_spent), apps) = tokio::join!(routing(env), clihub::self_check(&env.http));
    // Provider calls stay sequential: a subscription rate-limits bursts, and the cache test needs a quiet prefix.
    let mut spent = 0;
    let mut rows = vec![jev, row("CLI-Anything", apps)];
    rows.push(row("Lazy tools (tokens exacts)", lazy(&llm, &mut spent).await));
    rows.push(row("RTK (tokens exacts)", rtk(&llm, &mut spent).await));
    rows.push(row("TOON vs JSON (tokens exacts)", toon(&llm, &mut spent).await));
    rows.push(row("Cache préfixe (2 requêtes réelles)", cache(env, &llm, &mut spent).await));
    rows.push(row("Style de sortie (A/B réel)", style(env, &llm, &mut spent).await));
    let counting = if llm.wire() == Wire::Anthropic { "comptages count_tokens gratuits" } else { "comptages à 1 token de sortie" };
    rows.push(row(
        "Coût du banc",
        Ok(format!("{spent} tokens moteur ({}) + {jev_spent} tokens Jev · {counting}", llm.model)),
    ));
    rows
}

/// Jev against the labelled cases, local heuristics alongside; schema bytes Jev's choices actually ship.
async fn routing(env: &Env) -> (Health, u64) {
    let name = "Jev · routage + lazy tools";
    let local = CASES.iter().filter(|c| graded(c, &router::local(c.query))).count();
    let Some(jev) = &env.jev else {
        let detail = format!("clé Jev absente : heuristique locale {local}/{} justes", CASES.len());
        return (Health { group: GROUP, name, ok: false, detail }, 0);
    };
    let answers = join_all(CASES.iter().map(|c| async {
        let mut conv = Conversation::default();
        conv.push(Message::user(c.query.into()));
        router::ask_jev(jev, &router::Input::of(&conv)).await
    }))
    .await;
    let all = savings::schema_bytes(&ToolKind::ALL.into_iter().collect());
    let (mut right, mut tokens, mut shipped, mut misses) = (0, 0, 0, String::new());
    for (case, answer) in CASES.iter().zip(answers) {
        match answer {
            Ok((route, t)) => {
                tokens += t;
                shipped += savings::schema_bytes(&route.tools());
                if graded(case, &route) {
                    right += 1;
                } else {
                    let _ = write!(misses, " · ✗ « {} » → {}", compress::clip(case.query, 32), route.skill.id());
                }
            }
            Err(e) => {
                let detail = format!("Jev en échec : {e:#} — routage local utilisé ({local}/{} justes)", CASES.len());
                return (Health { group: GROUP, name, ok: false, detail }, tokens);
            }
        }
    }
    let n = CASES.len() as u64;
    let detail = format!(
        "Jev {right}/{n} justes (local {local}/{n}) · {} tokens/appel · schémas envoyés {} o en moyenne vs {all} o (−{} %){misses}",
        tokens / n,
        shipped / n,
        100 - shipped * 100 / (all * n).max(1)
    );
    (Health { group: GROUP, name, ok: right + 1 >= n, detail }, tokens)
}

/// Exact prompt size of one user text with these tools.
async fn count(llm: &Llm, text: &str, tools: &BTreeSet<ToolKind>, spent: &mut u64) -> Result<u64> {
    let messages = [json!({ "role": "system", "content": "Count." }), json!({ "role": "user", "content": text })];
    let tools = (!tools.is_empty()).then(|| schema::schemas(tools));
    let n = llm.count(&llm.body(&messages, tools)).await?;
    if llm.wire() == Wire::OpenAi {
        *spent += n + 1;
    }
    Ok(n)
}

async fn lazy(llm: &Llm, spent: &mut u64) -> Result<String> {
    let none = BTreeSet::new();
    let all = count(llm, "ok", &ToolKind::ALL.into_iter().collect(), spent).await?;
    let code = count(llm, "ok", &router::local("corrige le bug dans main.rs").tools(), spent).await?;
    let chat = count(llm, "ok", &none, spent).await?;
    ensure!(chat < code && code < all, "aucun gain mesuré ({chat} / {code} / {all} tokens)");
    let schemas = (all - chat).max(1);
    Ok(format!(
        "16 outils {all} tok · tour code {code} · tour simple {chat} → −{} tok/requête en code (−{} % des schémas), −{} en chat",
        all - code,
        (all - code) * 100 / schemas,
        all - chat
    ))
}

async fn rtk(llm: &Llm, spent: &mut u64) -> Result<String> {
    let raw = sample_log();
    let packed = compress::shell(&raw);
    let none = BTreeSet::new();
    let (a, b) = (count(llm, &raw, &none, spent).await?, count(llm, &packed, &none, spent).await?);
    ensure!(b < a, "aucune réduction ({a} → {b} tokens)");
    Ok(format!(
        "log cargo test de {} lignes : {a} → {b} tokens (−{} %), erreur et résumé conservés",
        raw.lines().count(),
        (a - b) * 100 / a
    ))
}

async fn toon(llm: &Llm, spent: &mut u64) -> Result<String> {
    let rows: Vec<Vec<String>> =
        (0..30).map(|i| vec![format!("src/module_{i}.rs"), (i * 7 + 3).to_string(), format!("pub fn handler_{i}(req: Request)")]).collect();
    let encoded = compress::toon("hits", &["file", "line", "code"], &rows);
    let as_json: Vec<Value> = rows.iter().map(|r| json!({ "file": r[0], "line": r[1], "code": r[2] })).collect();
    let none = BTreeSet::new();
    let (j, t) = (count(llm, &serde_json::to_string(&as_json)?, &none, spent).await?, count(llm, &encoded, &none, spent).await?);
    ensure!(t < j, "TOON pas plus court ({t} vs {j} tokens)");
    Ok(format!("30 résultats grep : JSON {j} → TOON {t} tokens (−{} %)", (j - t) * 100 / j))
}

fn used(spent: &mut u64, done: &Completion) -> Usage {
    *spent += done.usage.prompt + done.usage.completion;
    done.usage
}

/// The real system prompt and every tool, sent twice: the second request must be read from the cache.
async fn cache(env: &Env, llm: &Llm, spent: &mut u64) -> Result<String> {
    let system = prompts::system(&env.settings, &env.workspace());
    let messages =
        [json!({ "role": "system", "content": system }), json!({ "role": "user", "content": "Health check: reply with the single word OK." })];
    let mut body = llm.body(&messages, Some(schema::schemas(&ToolKind::ALL.into_iter().collect())));
    body["max_tokens"] = json!(if llm.wire() == Wire::Anthropic { 2048 } else { 16 });
    let first = used(spent, &llm.stream(&body, |_| {}).await?);
    // DeepSeek persists its disk cache asynchronously.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let second = used(spent, &llm.stream(&body, |_| {}).await?);
    ensure!(
        second.cached > 0,
        "2e requête identique : 0 token lu du cache sur {} — préfixe non mis en cache (taille minimale, compte ou TTL)",
        second.prompt
    );
    let r = Rates::of(llm.engine, &llm.model);
    let fresh = second.prompt.saturating_sub(second.cached + second.written) as f64;
    let billed = fresh + second.cached as f64 * r.read + second.written as f64 * r.write;
    Ok(format!(
        "1re : {} tok (écrits {}, lus {}) · 2e : {}/{} lus du cache ({:.0} %) → facturée {:.0} % du plein tarif",
        first.prompt,
        first.written,
        first.cached,
        second.cached,
        second.prompt,
        second.cached as f64 * 100.0 / second.prompt.max(1) as f64,
        billed * 100.0 / second.prompt.max(1) as f64
    ))
}

/// The same question under the Normal style and the active one; visible answer tokens compared.
async fn style(env: &Env, llm: &Llm, spent: &mut u64) -> Result<String> {
    let active = env.settings.style;
    if active == Style::Normal {
        return Ok("style Normal actif : aucune réduction de sortie (Comportement → Style de sortie)".into());
    }
    let question = "Explique ce qu'est un mutex, quand le préférer à un canal, avec un court exemple Rust.";
    let mut visible = [0_u64; 2];
    let mut billed = [0_u64; 2];
    for (k, s) in [Style::Normal, active].into_iter().enumerate() {
        let system = format!("You are Omega, an expert software engineer. Reply in the user's language.\n{}", s.prompt());
        let messages = [json!({ "role": "system", "content": system }), json!({ "role": "user", "content": question })];
        let done = llm.stream(&llm.body(&messages, None), |_| {}).await?;
        billed[k] = used(spent, &done).completion;
        visible[k] = answer_tokens(llm, &done, spent).await?;
    }
    ensure!(visible[1] < visible[0], "{} pas plus court que Normal ({} vs {} tokens)", active.label(), visible[1], visible[0]);
    Ok(format!(
        "réponse visible : Normal {} → {} {} tokens (−{} %) · sortie facturée {} → {} (raisonnement inclus) · 1 échantillon",
        visible[0],
        active.label(),
        visible[1],
        (visible[0] - visible[1]) * 100 / visible[0].max(1),
        billed[0],
        billed[1]
    ))
}

/// Tokens of the visible answer: counted exactly on Claude (its output usage includes thinking), usage minus reasoning elsewhere.
async fn answer_tokens(llm: &Llm, done: &Completion, spent: &mut u64) -> Result<u64> {
    if llm.wire() == Wire::OpenAi {
        return Ok(done.usage.completion.saturating_sub(done.usage.reasoning));
    }
    let none = BTreeSet::new();
    let with = count(llm, &format!("x {}", done.content), &none, spent).await?;
    let without = count(llm, "x", &none, spent).await?;
    Ok(with.saturating_sub(without))
}

/// A realistic `cargo test` run: build noise, 60 passing tests, one failure with its panic.
fn sample_log() -> String {
    const CRATES: [&str; 24] = [
        "proc-macro2 v1.0.95", "unicode-ident v1.0.18", "libc v0.2.174", "cfg-if v1.0.1", "memchr v2.7.5", "once_cell v1.21.3",
        "serde v1.0.219", "quote v1.0.40", "syn v2.0.104", "serde_derive v1.0.219", "itoa v1.0.15", "ryu v1.0.20",
        "serde_json v1.0.141", "pin-project-lite v0.2.16", "bytes v1.10.1", "mio v1.0.4", "socket2 v0.6.0", "tokio-macros v2.5.0",
        "tokio v1.47.1", "regex-syntax v0.8.5", "aho-corasick v1.1.3", "regex-automata v0.4.9", "regex v1.11.1", "omega v0.5.0",
    ];
    let mut log = String::from("\x1b[1m\x1b[32m    Updating\x1b[0m crates.io index\n");
    for c in CRATES {
        let _ = writeln!(log, "\x1b[1m\x1b[32m   Compiling\x1b[0m {c}");
    }
    log.push_str("    Finished `test` profile [unoptimized + debuginfo] target(s) in 41.27s\n");
    log.push_str("     Running unittests src/main.rs (target/debug/deps/omega-3f2a9c1d)\n\nrunning 61 tests\n");
    for i in 0..60 {
        let _ = writeln!(log, "test tools::module_{}::case_{i} ... ok", i % 7);
    }
    log.push_str("test tools::parser::parses_dates ... FAILED\n\nfailures:\n\n---- tools::parser::parses_dates stdout ----\n");
    log.push_str("thread 'tools::parser::parses_dates' panicked at src/tools/parser.rs:42:17:\nindex out of bounds: the len is 3 but the index is 3\n");
    log.push_str("note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\n\nfailures:\n    tools::parser::parses_dates\n\n");
    log.push_str("test result: FAILED. 60 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.84s\n");
    log
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_routing_grades_and_rtk_keeps_the_failure() {
        let packed = compress::shell(&sample_log());
        assert!(packed.contains("index out of bounds") && packed.contains("test result: FAILED"));
        assert!(!packed.contains("Compiling") && !packed.contains("... ok"));
        assert!(packed.len() * 3 < sample_log().len(), "{} vs {}", packed.len(), sample_log().len());
        let local = CASES.iter().filter(|c| graded(c, &router::local(c.query))).count();
        assert!(local >= 4, "heuristique locale {local}/7");
    }
}

#[cfg(test)]
mod live {
    /// Real provider calls with the saved settings: `cargo test -- --ignored bench_live --nocapture`.
    #[tokio::test]
    #[ignore = "network, spends tokens"]
    async fn bench_live() {
        let (settings, _) = crate::infra::store::load_settings();
        let web = std::sync::Arc::new(crate::tools::webindex::WebIndex::open(&crate::infra::paths::web_cache()));
        let env = super::Env::new(settings, web).unwrap();
        for r in super::run(&env).await {
            println!("[{}] {} — {}", if r.ok { "OK" } else { "KO" }, r.name, r.detail);
        }
    }
}
