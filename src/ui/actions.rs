//! User intents: send, stop, regenerate, edit, undo files, export, settings, background tasks.

use std::{fmt::Write as _, sync::Arc, time::Duration};

use uuid::Uuid;

use super::{App, Live, attach, build_env};
use crate::{
    app::{Env, Event, agent, health, prompts},
    domain::{
        auth::{AuthMode, Cred, Provider},
        chat::{Conversation, Message, Role},
        engine::{Efforts, Engine},
        schedule::Job,
        settings::Settings,
    },
    infra::{store, vpn},
    tools::checkpoint::Checkpoint,
};

/// The settings the self-tests depend on: credentials, endpoints, models, proxy, workspace, output style.
fn probed(s: &Settings) -> Settings {
    Settings {
        engine: Engine::default(),
        effort: Efforts::default(),
        max_steps: 0,
        unlimited_steps: false,
        auto_model: false,
        auto_approve: false,
        bypass: false,
        confirm_edits: false,
        auto_diagnostics: false,
        auto_effort: false,
        delegate: false,
        compact_at: 0,
        require_vpn: false,
        ip_lookup: false,
        ..s.clone()
    }
}

impl App {
    pub(super) fn new_chat(&mut self) {
        self.current = None;
        self.editing = None;
        self.draft.clear();
        self.focus_composer = true;
    }

    pub(super) fn select(&mut self, id: Uuid) {
        self.current = Some(id);
        self.editing = None;
        self.focus_composer = true;
    }

    pub(super) fn delete_chat(&mut self, id: Uuid) {
        self.stop(id);
        self.convs.retain(|c| c.id != id);
        self.errors.remove(&id);
        let _ = store::delete_conversation(id);
        if self.current == Some(id) {
            self.current = self.convs.first().map(|c| c.id);
        }
    }

    pub(super) fn send(&mut self) {
        let text = self.draft.trim().to_owned();
        if self.slash_command(&text) {
            return;
        }
        if (text.is_empty() && self.shots.is_empty()) || self.busy() {
            return;
        }
        if self.settings.require_vpn && !self.vpn.as_ref().is_some_and(|v| v.active) {
            self.notify("Kill-switch VPN : aucun VPN détecté, envoi bloqué.");
            return;
        }
        let images = std::mem::take(&mut self.shots);
        if !images.is_empty() && !self.settings.engine.vision() {
            self.notify(format!(
                "{} ne lit pas les images : elles restent locales (utilisez Claude pour les montrer).",
                self.settings.engine.label()
            ));
        }
        let id = if let Some(id) = self.current {
            id
        } else {
            let conv = Conversation::default();
            let id = conv.id;
            self.convs.insert(0, conv);
            self.current = Some(id);
            id
        };
        if let Some((conv, idx)) = self.editing.take().filter(|(c, _)| *c == id) {
            self.undo_from(conv, idx);
            if let Some(c) = self.conv_mut(conv) {
                c.truncate(idx);
            }
        }
        let Some(conv) = self.conv_mut(id) else { return };
        conv.repair();
        conv.push(if images.is_empty() { Message::user(text) } else { Message::user_with(text, images) });
        self.draft.clear();
        self.launch(id);
    }

    /// Composer commands handled locally, never sent to a model. `true` when `text` was one.
    fn slash_command(&mut self, text: &str) -> bool {
        match text {
            // Like Claude Code: a fresh, empty context. The previous conversation stays in the history.
            "/clear" | "/new" => {
                if self.busy() {
                    self.notify("Arrêtez le tour en cours avant /clear");
                    return true;
                }
                self.new_chat();
                self.shots.clear();
                self.notify("Contexte vidé : nouvelle discussion (l'ancienne reste dans l'historique)");
                true
            }
            "/tasks" | "/taches" | "/tâches" => {
                self.draft.clear();
                self.show_tasks = true;
                true
            }
            _ => false,
        }
    }

    /// Re-runs the last turn from its user message, restoring the files it changed first.
    pub(super) fn regenerate(&mut self, id: Uuid) {
        if self.live.contains_key(&id) {
            return;
        }
        let Some(idx) = self.conv(id).and_then(|c| c.messages.iter().rposition(|m| m.role == Role::User)) else { return };
        self.undo_from(id, idx);
        if let Some(c) = self.conv_mut(id) {
            c.truncate(idx + 1);
        }
        self.launch(id);
    }

    fn launch(&mut self, id: Uuid) {
        self.launch_with(id, self.env.clone());
    }

    /// Runs the conversation's last turn with `env` (the user's, or a scheduled job's own).
    fn launch_with(&mut self, id: Uuid, env: Arc<Env>) {
        let Some(snapshot) = self.conv(id).cloned() else { return };
        self.errors.remove(&id);
        self.save(id);
        self.convs.sort_by_key(|c| std::cmp::Reverse(c.updated));
        let (engine, model) = (env.llm.engine, env.llm.model.clone());
        let handle = self.spawn(agent::run(snapshot, env, self.emit.clone()));
        self.live.insert(id, Live { handle: Some(handle), engine, model, ..Live::default() });
    }

    /// Loads a user message into the composer; sending replaces it and everything after.
    pub(super) fn start_edit(&mut self, id: Uuid, idx: usize) {
        if self.live.contains_key(&id) {
            return;
        }
        if let Some(text) = self.conv(id).and_then(|c| c.messages.get(idx)).map(|m| m.content.clone()) {
            self.draft = text;
            self.editing = Some((id, idx));
            self.focus_composer = true;
        }
    }

    pub(super) fn cancel_edit(&mut self) {
        self.editing = None;
        self.draft.clear();
    }

    /// Restores files changed by every turn from message `idx` on (newest first).
    pub(super) fn undo_from(&mut self, id: Uuid, idx: usize) {
        let Some(conv) = self.conv_mut(id) else { return };
        let (mut restored, mut failure) = (0, None);
        for m in conv.messages.iter_mut().skip(idx).rev().filter(|m| m.checkpoint) {
            match Checkpoint::new(id, m.turn_key()).restore() {
                Ok(n) => {
                    restored += n;
                    m.checkpoint = false;
                }
                Err(e) => {
                    failure = Some(format!("Restauration incomplète : {e:#}"));
                    break;
                }
            }
        }
        if let Some(text) = failure.or_else(|| (restored > 0).then(|| format!("↶ {restored} fichier(s) restauré(s)"))) {
            self.notify(text);
        }
        self.save(id);
    }

    pub(super) fn export(&mut self, id: Uuid) {
        let Some(conv) = self.conv(id) else { return };
        let name: String =
            conv.title.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
        let Some(path) =
            rfd::FileDialog::new().set_file_name(format!("{}.md", name.trim())).add_filter("Markdown", &["md"]).save_file()
        else {
            return;
        };
        let text = match std::fs::write(&path, markdown(conv)) {
            Ok(()) => format!("Exporté : {}", path.display()),
            Err(e) => format!("Export impossible : {e}"),
        };
        self.notify(text);
    }

    /// Aborts a running turn, keeping what was streamed so far.
    pub(super) fn stop(&mut self, id: Uuid) {
        let Some(live) = self.live.remove(&id) else { return };
        if let Some(h) = live.handle {
            h.abort();
        }
        self.approvals.retain(|a| a.conv != id);
        if let Some(conv) = self.conv_mut(id) {
            if !live.text.is_empty() || !live.reasoning.is_empty() {
                conv.push(Message::assistant(live.text, live.reasoning, Vec::new()).by(live.engine, &live.model));
            }
            conv.repair();
            let turn = conv.messages.iter().rev().find(|m| m.role == Role::User).map(Message::turn_key);
            if let Some(turn) = turn.filter(|t| Checkpoint::new(id, *t).exists())
                && let Some(m) = conv.messages.iter_mut().find(|m| m.turn_key() == turn)
            {
                m.checkpoint = true;
            }
        }
        self.save(id);
    }

    pub(super) fn apply_settings(&mut self, settings: Settings) {
        if let Err(e) = store::save_settings(&settings) {
            self.notify(format!("Paramètres non enregistrés : {e:#}"));
        }
        let (env, notice) = build_env(&settings, &self.web);
        // Engine, effort or bypass switches do not re-run the network self-tests (token refreshes, paid probes).
        let probes = probed(&self.settings) != probed(&settings);
        let vpn = (&self.settings.proxy, self.settings.ip_lookup) != (&settings.proxy, settings.ip_lookup);
        // Watchers belong to one workspace and only run while the option is on.
        if !settings.live_tests || settings.workspace != self.settings.workspace {
            crate::tools::livetest::stop_all();
        }
        self.settings = settings;
        self.env = env;
        if let Some(n) = notice {
            self.notify(n);
        }
        if probes {
            self.refresh_health();
        }
        if vpn {
            self.restart_vpn();
        }
    }

    /// Opens the provider page and starts the OAuth dance.
    pub(super) fn connect(&mut self, provider: Provider) {
        self.settings_draft = None;
        self.cancel_login();
        let (env, emit) = (self.env.clone(), self.emit.clone());
        match provider {
            Provider::Claude => {
                let (url, login) = crate::app::login::claude_start();
                let _ = open::that(&url);
                self.login = Some(login);
            }
            Provider::Gemini => match crate::app::login::gemini_start() {
                Ok((url, login)) => {
                    let _ = open::that(&url);
                    let waiting = login.clone();
                    self.login_task = Some(self.spawn(async move {
                        let result = crate::app::login::await_gemini(&env.http, &waiting).await;
                        emit.send(Event::Connected { provider, result: result.map_err(|e| format!("{e:#}")) });
                    }));
                    self.login = Some(login);
                }
                Err(e) => self.notify(format!("Gemini : {e:#}")),
            },
            Provider::GitHub => {
                self.login = Some(crate::app::login::github_start());
                self.login_task = Some(self.spawn(async move {
                    let result = async {
                        let device = crate::infra::auth::github::start(&env.http).await?;
                        let _ = open::that(&device.verification_uri);
                        emit.send(Event::DeviceCode { code: device.user_code.clone(), url: device.verification_uri.clone() });
                        crate::infra::auth::github::wait(&env.http, &device).await
                    }
                    .await;
                    emit.send(Event::Connected { provider, result: result.map_err(|e| format!("{e:#}")) });
                }));
            }
        }
    }

    /// A pasted GitHub token (`ghp_…`, `github_pat_…`), checked against the API before it is kept.
    pub(super) fn use_github_token(&mut self) {
        let token = std::mem::take(&mut self.github_draft);
        let (env, emit) = (self.env.clone(), self.emit.clone());
        self.spawn(async move {
            let result = crate::infra::auth::github::import_token(&env.http, &token).await;
            emit.send(Event::Connected { provider: Provider::GitHub, result: result.map_err(|e| format!("{e:#}")) });
        });
    }

    /// Installs the Antigravity CLI in the background (Google's installer, no window); `login` then opens the
    /// sign-in console.
    pub(super) fn install_agy(&mut self, login: bool) {
        if self.agy_installing {
            return;
        }
        self.agy_installing = true;
        self.notify("Installation d'Antigravity (agy) en arrière-plan…");
        let emit = self.emit.clone();
        self.spawn(async move {
            let result = crate::infra::llm::agy::install().await.map(|p| p.display().to_string()).map_err(|e| format!("{e:#}"));
            emit.send(Event::AgyInstalled { result, login });
        });
    }

    pub(super) fn agy_installed(&mut self, result: Result<String, String>, login: bool) {
        self.agy_installing = false;
        match result {
            Ok(path) => {
                // The environment re-detects agy: Gemini now runs through it.
                self.apply_settings(self.settings.clone());
                if login {
                    self.agy_login();
                } else {
                    self.notify(format!("Antigravity installé : {path}"));
                }
            }
            Err(e) => self.notify(format!("Antigravity non installé : {e}")),
        }
    }

    /// Opens `agy` in a console for the Google sign-in.
    pub(super) fn agy_login(&mut self) {
        let opened = crate::infra::llm::agy::binary()
            .ok_or_else(|| anyhow::anyhow!("agy n'est pas installé"))
            .and_then(|bin| crate::infra::llm::agy::open_login(&bin));
        match opened {
            Ok(()) => self.notify("Antigravity : connectez-vous dans la page Google, puis /quit dans la console"),
            Err(e) => self.notify(format!("Antigravity : {e:#}")),
        }
    }

    pub(super) fn disconnect(&mut self, provider: Provider) {
        match crate::app::login::forget(provider) {
            Ok(()) => {
                self.reload_accounts();
                self.plan_usage.remove(&provider);
                // The environment re-reads the credentials: a removed token stops reaching requests and shells.
                self.apply_settings(self.settings.clone());
                self.notify(format!("{} déconnecté", provider.label()));
            }
            Err(e) => self.notify(format!("Déconnexion impossible : {e:#}")),
        }
    }

    /// Exchanges the code the user pasted from the provider page.
    pub(super) fn submit_code(&mut self) {
        let Some(mut login) = self.login.clone() else { return };
        if login.paste.trim().is_empty() || login.submitting {
            return;
        }
        login.submitting = true;
        login.status = "Validation…".to_owned();
        self.login = Some(login.clone());
        let (env, emit, provider) = (self.env.clone(), self.emit.clone(), login.provider);
        self.spawn(async move {
            let result = crate::app::login::finish_paste(&env.http, &login, login.paste.trim()).await;
            emit.send(Event::Connected { provider, result: result.map_err(|e| format!("{e:#}")) });
        });
    }

    /// Closes the modal and stops whatever waits for the provider (loopback listener, device polling).
    pub(super) fn cancel_login(&mut self) {
        self.login = None;
        if let Some(task) = self.login_task.take() {
            task.abort();
        }
    }

    /// Imports the session the official Claude Code CLI already logged in.
    pub(super) fn import_claude(&mut self) {
        let result = crate::app::login::import_cli();
        self.connected(Provider::Claude, result.map_err(|e| format!("{e:#}")));
    }

    /// Long-lived `sk-ant-oat…` token typed in the settings: no OAuth exchange, no rate limit.
    pub(super) fn use_token(&mut self) {
        let token = std::mem::take(&mut self.token_draft);
        let result = crate::infra::auth::claude::import_token(&token).map_err(|e| format!("{e:#}"));
        self.connected(Provider::Claude, result);
    }

    /// Persists fresh credentials, switches the provider to subscription mode and selects it.
    pub(super) fn connected(&mut self, provider: Provider, result: Result<Cred, String>) {
        match result {
            Ok(cred) => {
                if let Err(e) = crate::app::login::persist(provider, &cred) {
                    self.notify(format!("Jetons non enregistrés : {e:#}"));
                    return;
                }
                let subscribe = |s: &mut Settings| match provider {
                    // Claude becomes the engine at once; Gemini first serves delegation, the main engine stays.
                    Provider::Claude => {
                        s.claude_auth = AuthMode::Subscription;
                        s.engine = Engine::Claude;
                    }
                    Provider::Gemini => s.gemini_auth = AuthMode::Subscription,
                    Provider::GitHub => {}
                };
                let mut settings = self.settings.clone();
                subscribe(&mut settings);
                // An open settings form must not put the old mode back when it is saved.
                if let Some(draft) = &mut self.settings_draft {
                    subscribe(draft);
                }
                // Always rebuilt: the environment re-reads the new credentials (shell token, Code Assist project).
                self.apply_settings(settings);
                self.reload_accounts();
                if let Some(engine) = provider.engine() {
                    self.refresh_models(engine);
                }
                self.cancel_login();
                let account = if cred.account.is_empty() { provider.label().to_owned() } else { cred.account.clone() };
                let note = match provider {
                    Provider::Gemini if self.settings.engine != Engine::Gemini => " — délégation active, moteur sélectionnable",
                    _ => "",
                };
                self.notify(format!("{} connecté · {account}{note}", provider.label()));
                self.fetch_plan_usage(provider);
            }
            Err(e) => {
                if let Some(login) = self.login.as_mut().filter(|l| l.provider == provider) {
                    login.submitting = false;
                    login.status = e;
                } else {
                    self.notify(e);
                }
            }
        }
    }

    /// Best-effort: any failure silently leaves the usage section hidden.
    pub(super) fn fetch_plan_usage(&mut self, provider: Provider) {
        let (env, emit) = (self.env.clone(), self.emit.clone());
        let _task = self.spawn(async move {
            let Some(auth) = crate::infra::auth::Auth::load(provider) else { return };
            let Ok(token) = auth.fresh(&env.http).await else { return };
            let windows = match provider {
                Provider::Claude => crate::infra::auth::claude::usage(&env.http, &token).await,
                Provider::Gemini => crate::infra::auth::gemini::usage(&env.http, &token, &auth.project()).await,
                Provider::GitHub => None,
            };
            if let Some(windows) = windows {
                emit.send(Event::PlanUsage { provider, windows });
            }
        });
    }

    pub(super) fn pick_workspace(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().set_title("Dossier de travail").pick_folder() {
            let settings = Settings { workspace: Some(dir), ..self.settings.clone() };
            self.apply_settings(settings);
        }
    }

    /// Writes a starter OMEGA.md in the workspace (never overwrites).
    pub(super) fn create_rules_file(&mut self) {
        let path = self.env.workspace().join(prompts::RULE_FILES[0]);
        let result = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, RULES_TEMPLATE.as_bytes()));
        match result {
            Ok(()) => {
                self.notify(format!("Créé : {} — complétez-le", path.display()));
                let _ = open::that(&path);
                self.refresh_health();
            }
            Err(e) => self.notify(format!("Création impossible : {e}")),
        }
    }

    pub(super) fn refresh_health(&mut self) {
        self.health.clear();
        let (env, emit) = (self.env.clone(), self.emit.clone());
        self.spawn(async move { emit.send(Event::Health(health::check(&env).await)) });
    }

    /// Live token-economy benchmark on the saved settings (real provider calls).
    pub(super) fn run_bench(&mut self) {
        self.bench_running = true;
        let (env, emit) = (self.env.clone(), self.emit.clone());
        self.spawn(async move { emit.send(Event::Bench(crate::app::bench::run(&env).await)) });
    }

    /// Asks the provider which model ids this credential may call; the picker merges them with the catalogue.
    pub(super) fn refresh_models(&mut self, engine: Engine) {
        let (env, emit) = (self.env.clone(), self.emit.clone());
        self.spawn(async move {
            let llm = crate::infra::llm::Llm::new(env.http.clone(), &env.settings, engine);
            let result = if llm.configured() {
                llm.catalog().await.map_err(|e| format!("{e:#}"))
            } else {
                Err("aucun identifiant connecté".to_owned())
            };
            emit.send(Event::Models { engine, result });
        });
    }

    /// Adds a screenshot to the composer, with a hard cap.
    pub(super) fn attach(&mut self, img: crate::domain::chat::Image) {
        if self.shots.len() >= attach::MAX_IMAGES {
            self.notify(format!("{} images maximum par message", attach::MAX_IMAGES));
            return;
        }
        self.shots.push(img);
    }

    /// Files dropped on the window or pasted from the clipboard.
    pub(super) fn collect_images(&mut self, ctx: &eframe::egui::Context) {
        if self.settings_draft.is_some() || self.login.is_some() || self.schedule_draft.is_some() {
            return;
        }
        let dropped = attach::dropped(ctx).into_iter().map(|b| b.and_then(|b| attach::from_bytes(&b)));
        for img in dropped.chain(attach::pasted(ctx)) {
            match img {
                Ok(img) => self.attach(img),
                Err(e) => self.notify(e),
            }
        }
    }

    /// Connected account of each subscription, for the settings form.
    pub(super) fn reload_accounts(&mut self) {
        self.accounts = Provider::ALL
            .into_iter()
            .filter_map(|p| Some((p, crate::infra::auth::Auth::load(p)?.account())))
            .filter(|(_, account)| !account.is_empty())
            .collect();
    }

    /// While the user types, the engine and Jev connections open (and a subscription token refreshes) ahead of the send.
    pub(super) fn warm_up(&mut self) {
        const EVERY: Duration = Duration::from_secs(240);
        if self.draft.trim().is_empty() || self.warmed.is_some_and(|at| at.elapsed() < EVERY) {
            return;
        }
        if self.settings.require_vpn && !self.vpn.as_ref().is_some_and(|v| v.active) {
            return;
        }
        self.warmed = Some(std::time::Instant::now());
        let env = self.env.clone();
        self.spawn(async move {
            let jev = async {
                if let Some(jev) = &env.jev {
                    jev.warm().await;
                }
            };
            let helper = async {
                if let Some(helper) = env.helpers.first() {
                    helper.warm().await;
                }
            };
            tokio::join!(env.llm.warm(), jev, helper);
        });
    }

    pub(super) fn restart_vpn(&mut self) {
        if let Some(task) = self.vpn_task.take() {
            task.abort();
        }
        let (env, emit) = (self.env.clone(), self.emit.clone());
        self.vpn_task = Some(self.spawn(async move {
            loop {
                emit.send(Event::Vpn(vpn::detect(&env.http, env.settings.ip_lookup).await));
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        }));
    }

    /// Opens the scheduler with the current draft as the default prompt.
    pub(super) fn open_schedule(&mut self) {
        let prompt = std::mem::take(&mut self.draft);
        let engine = self.settings.engine;
        self.schedule_draft = Some(super::schedule::Draft::new(
            prompt,
            engine,
            self.settings.model(engine).to_owned(),
            self.settings.effort.get(engine),
        ));
        self.schedule_error = None;
    }

    pub(super) fn persist_jobs(&mut self) {
        if let Err(e) = crate::infra::store::save_jobs(&self.jobs) {
            self.notify(format!("Programmation non enregistrée : {e:#}"));
        }
    }

    /// Fires every job whose instant has passed. Each one starts its own conversation, in bypass.
    pub(super) fn fire_due_jobs(&mut self) {
        // Kill-switch on: wait for the first VPN detection instead of discarding what is due at startup.
        if self.settings.require_vpn && self.vpn.is_none() {
            return;
        }
        let now = chrono::Utc::now();
        let due: Vec<Uuid> = self.jobs.iter().filter(|j| j.due(now)).map(|j| j.id).collect();
        if due.is_empty() {
            return;
        }
        for id in due {
            self.run_job(id, now);
        }
        self.persist_jobs();
    }

    /// Tasks tab "Lancer maintenant": runs the job once without moving its schedule.
    pub(super) fn run_job_now(&mut self, id: Uuid) {
        let Some(job) = self.jobs.iter().find(|j| j.id == id).cloned() else { return };
        if let Some(conv) = self.start_job(&job) {
            if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                j.runs.push(crate::domain::schedule::Run { at: chrono::Utc::now(), conv });
            }
            self.persist_jobs();
        }
    }

    /// Fires a due job and records the run (one-shot: done; daily: next occurrence).
    fn run_job(&mut self, id: Uuid, now: chrono::DateTime<chrono::Utc>) {
        let Some(job) = self.jobs.iter().find(|j| j.id == id).cloned() else { return };
        let conv = self.start_job(&job);
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
            match conv {
                Some(conv) => j.ran(conv, now),
                // Not launched (VPN kill-switch): a daily job still moves on, a one-shot one waits for the next frame.
                None if j.daily => j.advance(now),
                None => j.paused = true,
            }
        }
    }

    /// A job runs with its own engine, model and effort, in bypass, through an environment of its own:
    /// the user's settings, draft, images and open conversation are left untouched. Returns the conversation.
    fn start_job(&mut self, job: &Job) -> Option<Uuid> {
        if self.settings.require_vpn && !self.vpn.as_ref().is_some_and(|v| v.active) {
            self.notify("Kill-switch VPN : tâche programmée non lancée (aucun VPN détecté) — mise en pause.");
            return None;
        }
        let mut settings = self.settings.clone();
        settings.engine = job.engine;
        if !job.model.trim().is_empty() {
            settings.set_model(job.engine, &job.model);
        }
        settings.effort.set(job.engine, job.effort);
        // A scheduled run never waits for a human: bypass is part of the contract.
        settings.bypass = true;
        let (env, _) = build_env(&settings, &self.web);
        let mut conv = Conversation::default();
        let id = conv.id;
        conv.push(Message::user(job.prompt.clone()));
        self.convs.insert(0, conv);
        self.launch_with(id, env);
        self.notify("Tâche programmée lancée (bypass)");
        Some(id)
    }
}

const RULES_TEMPLATE: &str = "# Règles du projet (lues par Omega à chaque requête)

## Projet
- But : …
- Stack : …

## Commandes
- Build : …
- Tests : …
- Lint : …

## Conventions
- …

## Interdits
- Ne jamais modifier : …
";

fn markdown(conv: &Conversation) -> String {
    let mut out = format!("# {}\n\n", conv.title);
    for m in &conv.messages {
        let _ = match m.role {
            Role::User => {
                let _ = write!(out, "## Vous\n\n{}\n\n", m.content);
                m.route.as_ref().map_or(Ok(()), |r| {
                    writeln!(
                        out,
                        "> 🧭 {:?} · skill {} · playbook {} · web {:.0}% · code {:.0}% · risque {:.0}% · impact {:.0}% · apps {:.0}%\n",
                        r.source,
                        r.skill.id(),
                        r.playbook.as_deref().unwrap_or("-"),
                        r.web * 100.0,
                        r.code * 100.0,
                        r.risk * 100.0,
                        r.blast * 100.0,
                        r.apps * 100.0
                    )
                })
            }
            Role::Assistant if m.content.trim().is_empty() && m.tool_calls.is_empty() => Ok(()),
            Role::Assistant => {
                let _ = write!(out, "## Omega\n\n{}\n\n", m.content.trim());
                m.tool_calls.iter().try_for_each(|c| writeln!(out, "> 🔧 `{}` {}", c.name, c.arguments))
            }
            Role::Tool => Ok(()),
        };
    }
    out
}
