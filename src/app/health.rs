//! Live self-tests: every tool is actually exercised, not just "configured".

use anyhow::Result;
use serde_json::json;

use super::{Env, prompts};
use crate::{
    domain::{engine::Engine, playbook},
    infra::{jev::Question, llm::Llm},
    tools::{clihub, code, compress, graph, guard, search, security, shell, web},
};

/// Name of the project-rules check (the UI offers to create the file when it fails).
pub const RULES_CHECK: &str = "Règles projet";

pub struct Health {
    pub group: &'static str,
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

fn entry(group: &'static str, name: &'static str, result: Result<String>) -> Health {
    match result {
        Ok(detail) => Health { group, name, ok: true, detail },
        Err(e) => Health { group, name, ok: false, detail: format!("{e:#}") },
    }
}

pub async fn check(env: &Env) -> Vec<Health> {
    let (kimi, deepseek, jev, ddg, sh, apps) = tokio::join!(
        engine(env, Engine::Kimi),
        engine(env, Engine::DeepSeek),
        jev(env),
        web::probe(&env.http),
        shell::probe(),
        clihub::probe(&env.http)
    );
    let local = tokio::task::spawn_blocking(|| {
        [
            code::self_test(),
            graph::self_test(),
            search::self_test(),
            compress::self_test(),
            security::self_test(),
            guard::self_test(),
        ]
    })
    .await;
    let [ts, gr, rg, rtk, sec, grd] = match local {
        Ok(results) => results,
        Err(e) => std::array::from_fn(|_| Err(anyhow::anyhow!("{e}"))),
    };
    let toon = compress::toon_self_test();
    let pages = env.web.len();
    let routing = if env.jev.is_some() { "Jev (System One)" } else { "heuristique locale" };
    vec![
        entry("Moteurs IA", Engine::Kimi.label(), kimi),
        entry("Moteurs IA", Engine::DeepSeek.label(), deepseek),
        entry("Moteurs IA", "Jev System One", jev),
        entry("Code", "Tree-sitter AST", ts),
        entry("Code", "Graphe d'impact", gr),
        entry("Code", "Moteur ripgrep", rg),
        entry("Code", "Shell", sh),
        entry("Économie tokens", "Compresseur RTK", rtk),
        entry("Économie tokens", "Encodeur TOON", toon),
        entry("Économie tokens", "Lazy tools", Ok(format!("routage {routing}"))),
        entry("Économie tokens", "Cache préfixe KV", Ok("préfixe statique, historique immuable".into())),
        entry("Économie tokens", "Style de sortie", Ok(env.settings.style.label().into())),
        entry("Skills", "Superpowers (obra)", playbooks()),
        entry("Skills", RULES_CHECK, project_rules(env)),
        entry("Apps", "CLI-Anything (HKUDS)", apps),
        entry("Web", "Recherche anonyme", ddg),
        entry("Web", "Web local (BM25)", Ok(format!("{pages} pages en cache"))),
        entry("Sécurité", "Scanner statique", sec),
        entry("Sécurité", "Garde shell + sandbox", grd),
    ]
}

fn project_rules(env: &Env) -> Result<String> {
    let (name, text) = prompts::project_rules(&env.workspace())
        .ok_or_else(|| anyhow::anyhow!("aucun {} à la racine du projet", prompts::RULE_FILES[0]))?;
    Ok(format!("{name} chargé ({} car.)", text.len()))
}

fn playbooks() -> Result<String> {
    let ok = playbook::ALL.iter().filter(|p| !p.description().is_empty() && p.body().len() > 500).count();
    anyhow::ensure!(ok == playbook::ALL.len(), "{ok}/{} playbooks valides", playbook::ALL.len());
    Ok(format!("{ok} playbooks · chargés à la demande par Jev"))
}

async fn engine(env: &Env, engine: Engine) -> Result<String> {
    let llm = Llm::new(env.http.clone(), &env.settings, engine);
    anyhow::ensure!(llm.configured(), "clé API absente ou URL non HTTPS");
    llm.ping().await?;
    let active = if engine == env.settings.engine { " · actif" } else { "" };
    Ok(format!("{} · effort {}{active}", llm.model, engine.effort()))
}

async fn jev(env: &Env) -> Result<String> {
    let jev = env.jev.as_ref().ok_or_else(|| anyhow::anyhow!("clé API absente — routage local"))?;
    let q = [("ok".to_owned(), Question::noul("Is this a health check?"))];
    let answers = jev.ask(json!("health check"), &q).await?;
    Ok(format!("{} tokens/appel", answers.tokens))
}
