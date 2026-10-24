//! Live self-tests: every tool is actually exercised, not just "configured".

use anyhow::Result;
use serde_json::json;

use super::{Env, prompts, savings};
use crate::ui::tr;
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
    let (deepseek, claude, gemini, jev, ddg, sh, apps) = tokio::join!(
        engine(env, Engine::DeepSeek),
        engine(env, Engine::Claude),
        engine(env, Engine::Gemini),
        jev(env),
        web::probe(&env.http),
        shell::probe(),
        clihub::probe(&env.http)
    );
    let web = env.web.clone();
    let local = tokio::task::spawn_blocking(move || {
        [
            code::self_test(),
            graph::self_test(),
            search::self_test(),
            compress::self_test(),
            security::self_test(),
            guard::self_test(),
            savings::prefix_self_test(&web),
            savings::lazy_self_test(),
        ]
    })
    .await;
    let [ts, gr, rg, rtk, sec, grd, prefix, lazy] = match local {
        Ok(results) => results,
        Err(e) => std::array::from_fn(|_| Err(anyhow::anyhow!("{e}"))),
    };
    let toon = compress::toon_self_test();
    let pages = env.web.len();
    let routing = if env.jev.is_some() { "Jev (System One)" } else { tr("heuristique locale", "local heuristics") };
    vec![
        entry("Moteurs IA", Engine::DeepSeek.label(), deepseek),
        entry("Moteurs IA", Engine::Claude.label(), claude),
        entry("Moteurs IA", Engine::Gemini.label(), gemini),
        entry("Moteurs IA", "Jev System One", jev),
        entry("Moteurs IA", "Délégation Flash", delegation(env)),
        entry("Code", "Tree-sitter AST", ts),
        entry("Code", "Graphe d'impact", gr),
        entry("Code", "Moteur ripgrep", rg),
        entry("Code", "Shell", sh),
        entry("Économie tokens", "Compresseur RTK", rtk),
        entry("Économie tokens", "Encodeur TOON", toon),
        entry("Économie tokens", "Lazy tools", lazy.map(|d| format!("{} {routing} · {d}", tr("routage", "routing")))),
        entry("Économie tokens", "Cache préfixe KV", prefix),
        entry("Économie tokens", "Style de sortie", Ok(env.settings.style.label().into())),
        entry("Skills", "Superpowers (obra)", playbooks()),
        entry("Skills", RULES_CHECK, project_rules(env)),
        entry("Apps", "CLI-Anything (HKUDS)", apps),
        entry("Apps", "Kits design & web", Ok(format!("{} kits · {}", crate::tools::kits::KITS.len(), tr("chargés par Jev à la demande", "loaded by Jev on demand")))),
        entry("Code", "Tests en continu", live_tests(env)),
        entry("Code", "GitHub", github(env)),
        entry("Web", "Recherche anonyme", ddg),
        entry("Web", "Web local (BM25)", Ok(format!("{pages} {}", tr("pages en cache", "cached pages")))),
        entry("Sécurité", "Scanner statique", sec),
        entry("Sécurité", "Garde shell + sandbox", grd),
        entry("Sécurité", "Skill audit (Cloudflare)", audit_skill()),
    ]
}

/// The chain delegated work goes down (`Gemini Flash → DeepSeek Flash`), the main engine closing it.
fn delegation(env: &Env) -> Result<String> {
    if !env.settings.delegate {
        return Ok(tr("désactivée", "off").to_owned());
    }
    anyhow::ensure!(!env.helpers.is_empty(), "{}", tr("aucun modèle Flash connecté (Gemini ou DeepSeek)", "no Flash model connected (Gemini or DeepSeek)"));
    let chain: Vec<String> = env.helpers.iter().map(|h| crate::domain::models::name(h.engine, &h.model)).collect();
    Ok(format!("{} → {}", chain.join(" → "), tr("moteur principal", "main engine")))
}

fn live_tests(env: &Env) -> Result<String> {
    if !env.settings.live_tests {
        return Ok(tr("désactivés (⚙ Paramètres)", "off (⚙ Settings)").to_owned());
    }
    let plugin = crate::tools::livetest::detect(&env.workspace())
        .ok_or_else(|| anyhow::anyhow!("{}", tr("aucun projet testable à la racine du dossier de travail", "no testable project at the workspace root")))?;
    let watcher = plugin.watch.as_ref().map_or(tr("watcher Omega", "Omega watcher"), |w| w.command);
    Ok(format!("{} · {watcher} {}", plugin.name, tr("(ou watcher Omega si absent)", "(or the Omega watcher if missing)")))
}

fn github(env: &Env) -> Result<String> {
    let auth = crate::infra::auth::Auth::load(crate::domain::auth::Provider::GitHub)
        .ok_or_else(|| anyhow::anyhow!("{}", tr("non connecté (⚙ Paramètres → GitHub)", "not connected (⚙ Settings → GitHub)")))?;
    let shared = if env.settings.github_shell {
        tr("git/gh du shell authentifiés", "shell git/gh signed in")
    } else {
        tr("non partagé avec le shell", "not shared with the shell")
    };
    Ok(format!("{} · {shared}", auth.account()))
}

fn project_rules(env: &Env) -> Result<String> {
    let (name, text) = prompts::project_rules(&env.workspace())
        .ok_or_else(|| anyhow::anyhow!("aucun {} à la racine du projet", prompts::RULE_FILES[0]))?;
    Ok(format!("{name} {} ({} {})", tr("chargé", "loaded"), text.len(), tr("car.", "chars")))
}

fn playbooks() -> Result<String> {
    let ok = playbook::ALL.iter().filter(|p| !p.description().is_empty() && p.body().len() > 500).count();
    anyhow::ensure!(ok == playbook::ALL.len(), "{ok}/{} playbooks valides", playbook::ALL.len());
    let generic = playbook::ALL.iter().filter(|p| p.group == playbook::Group::Superpowers).count();
    Ok(format!(
        "{generic} playbooks {} + 1 {} (Cloudflare) · {}",
        playbook::Group::Superpowers.label(),
        playbook::Group::Security.label(),
        tr("chargés à la demande par Jev", "loaded on demand by Jev")
    ))
}

/// The audit skill is embedded: green even without Node, which only gates the JSON validators.
fn audit_skill() -> Result<String> {
    let files = crate::tools::audit::FILES.len();
    anyhow::ensure!(files >= 20, "bundle incomplet : {files} fichiers");
    let node = crate::infra::process::blocking("node").arg("--version").output().is_ok_and(|o| o.status.success());
    Ok(format!(
        "{files} {} · {}: node {}",
        tr("fichiers", "files"),
        tr("validateurs", "validators"),
        if node { tr("détecté", "found") } else { tr("absent (lecture du schéma)", "missing (schema read only)") }
    ))
}

async fn engine(env: &Env, engine: Engine) -> Result<String> {
    let llm = Llm::new(env.http.clone(), &env.settings, engine);
    anyhow::ensure!(llm.configured(), "{}", tr("aucun identifiant (clé API ou abonnement)", "no credentials (API key or subscription)"));
    llm.ping().await?;
    // Engine and effort switches do not rerun the checks: they are shown live in the « Moteur actif » card.
    let mode = if llm.subscription() { tr("abonnement", "subscription") } else { tr("clé API", "API key") };
    Ok(format!("{} · {mode}", llm.model))
}

async fn jev(env: &Env) -> Result<String> {
    let jev = env.jev.as_ref().ok_or_else(|| anyhow::anyhow!("{}", tr("clé API absente — routage local", "no API key — local routing")))?;
    let q = [("ok".to_owned(), Question::noul("Is this a health check?"))];
    let answers = jev.ask(json!("health check"), &q).await?;
    Ok(format!("{} tokens/{}", answers.tokens, tr("appel", "call")))
}

#[cfg(test)]
mod tests {
    #[test]
    fn audit_skill_health_is_green_without_node() {
        let detail = super::audit_skill().expect("bundle présent");
        assert!(detail.contains("fichiers"));
        assert!(detail.contains("node") || detail.contains("validateurs"));
    }

    #[test]
    fn playbooks_health_counts_both_groups() {
        let detail = super::playbooks().expect("playbooks présents");
        assert!(detail.contains("15 playbooks Superpowers"), "{detail}");
        assert!(detail.contains("Audit sécurité"), "{detail}");
    }

    /// Every probe with the saved settings: `cargo test -- --ignored health_live --nocapture`.
    #[tokio::test]
    #[ignore = "network"]
    async fn health_live() {
        let (settings, _) = crate::infra::store::load_settings();
        let web = std::sync::Arc::new(crate::tools::webindex::WebIndex::open(&crate::infra::paths::web_cache()));
        let env = super::Env::new(settings, web).unwrap();
        for h in super::check(&env).await {
            println!("[{}] {} · {} — {}", if h.ok { "OK" } else { "KO" }, h.group, h.name, h.detail);
        }
    }
}
