//! User intents: send, stop, regenerate, edit, undo files, export, settings, background tasks.

use std::{fmt::Write as _, time::Duration};

use uuid::Uuid;

use super::{App, Live, attach, build_env};
use crate::{
    app::{Event, agent, health, prompts},
    domain::{
        auth::{AuthMode, Cred, Provider},
        chat::{Conversation, Message, Role},
        settings::Settings,
    },
    infra::{store, vpn},
    tools::checkpoint::Checkpoint,
};

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
                "{} ne lit pas les images : elles restent locales (utilisez Claude ou Gemini pour les montrer).",
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
        let Some(snapshot) = self.conv(id).cloned() else { return };
        self.errors.remove(&id);
        self.save(id);
        self.convs.sort_by_key(|c| std::cmp::Reverse(c.updated));
        let handle = self.spawn(agent::run(snapshot, self.env.clone(), self.emit.clone()));
        self.live.insert(id, Live { handle: Some(handle), ..Live::default() });
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
                conv.push(Message::assistant(live.text, live.reasoning, Vec::new()));
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
        self.settings = settings;
        self.env = env;
        if let Some(n) = notice {
            self.notify(n);
        }
        self.restart_background();
    }

    /// Opens the provider page and starts the OAuth dance.
    pub(super) fn connect(&mut self, provider: Provider) {
        self.settings_draft = None;
        let started = match provider {
            Provider::Claude => Ok(crate::app::login::claude_start()),
            Provider::Gemini => crate::app::login::gemini_start(),
        };
        match started {
            Ok((url, login)) => {
                let _ = open::that(&url);
                if provider == Provider::Gemini {
                    let (env, emit, task_login) = (self.env.clone(), self.emit.clone(), login.clone());
                    let task = self.spawn(async move {
                        let result = crate::app::login::await_gemini(&env.http, &task_login).await;
                        emit.send(Event::Connected { provider, result: result.map_err(|e| format!("{e:#}")) });
                    });
                    self.login_task = Some(task);
                }
                self.login = Some(login);
            }
            Err(e) => self.notify(format!("Connexion {} impossible : {e:#}", provider.label())),
        }
    }

    /// Exchanges the code the user pasted from the provider page.
    pub(super) fn submit_code(&mut self) {
        let Some(login) = self.login.clone() else { return };
        if login.paste.trim().is_empty() {
            return;
        }
        let (env, emit, provider) = (self.env.clone(), self.emit.clone(), login.provider);
        self.spawn(async move {
            let result = crate::app::login::finish_paste(&env.http, &login, login.paste.trim()).await;
            emit.send(Event::Connected { provider, result: result.map_err(|e| format!("{e:#}")) });
        });
    }

    pub(super) fn cancel_login(&mut self) {
        if let Some(task) = self.login_task.take() {
            task.abort();
        }
        self.login = None;
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
                let mut settings = self.settings.clone();
                match provider {
                    Provider::Claude => settings.claude_auth = AuthMode::Subscription,
                    Provider::Gemini => settings.gemini_auth = AuthMode::Subscription,
                }
                settings.engine = provider.engine();
                self.apply_settings(settings);
                if let Some(task) = self.login_task.take() {
                    task.abort();
                }
                self.login = None;
                let account = if cred.account.is_empty() { provider.label().to_owned() } else { cred.account.clone() };
                self.notify(format!("{} connecté · {account}", provider.label()));
                if provider == Provider::Claude {
                    self.fetch_plan_usage();
                }
            }
            Err(e) => {
                if let Some(login) = &mut self.login {
                    login.status = e;
                } else {
                    self.notify(e);
                }
            }
        }
    }

    /// Best-effort: any failure silently leaves the usage section hidden.
    fn fetch_plan_usage(&mut self) {
        let (env, emit) = (self.env.clone(), self.emit.clone());
        let _task = self.spawn(async move {
            let Some(auth) = crate::infra::auth::Auth::load(Provider::Claude) else { return };
            let Ok(token) = auth.fresh(&env.http).await else { return };
            if let Some(windows) = crate::infra::auth::claude::usage(&env.http, &token).await {
                emit.send(Event::PlanUsage { provider: Provider::Claude, windows });
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
        for bytes in attach::dropped(ctx) {
            match attach::from_bytes(&bytes) {
                Ok(img) => self.attach(img),
                Err(e) => self.notify(e),
            }
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(eframe::egui::Key::V))
            && let Some(img) = attach::from_clipboard()
        {
            self.attach(img);
        }
    }

    pub(super) fn restart_background(&mut self) {
        self.refresh_health();
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
        self.schedule_draft = Some(super::schedule::Draft::new(
            prompt,
            self.settings.engine,
            self.settings.effort.get(self.settings.engine),
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
        let now = chrono::Utc::now();
        let due: Vec<crate::domain::schedule::Job> = self.jobs.iter().filter(|j| j.due(now)).cloned().collect();
        if due.is_empty() {
            return;
        }
        self.jobs.retain_mut(|j| {
            if j.due(now) {
                if j.daily {
                    j.advance(now);
                    return true;
                }
                return false;
            }
            true
        });
        self.persist_jobs();
        for job in due {
            let mut settings = self.settings.clone();
            settings.engine = job.engine;
            settings.effort.set(job.engine, job.effort);
            // A scheduled run never waits for a human: bypass is part of the contract.
            settings.bypass = true;
            self.apply_settings(settings);
            self.new_chat();
            self.notify("Tâche programmée lancée (bypass)");
            self.send_text(job.prompt);
        }
    }
}

impl App {
    /// Same path as `send`, minus the composer.
    pub(super) fn send_text(&mut self, text: String) {
        self.draft = text;
        self.send();
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
