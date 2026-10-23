//! egui front-end: owns conversations, bridges UI ↔ tokio runtime through events.

mod actions;
mod cards;
mod chat;
mod composer;
mod events;
mod panel;
mod settings;
mod sidebar;
mod theme;
mod widgets;

use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use eframe::egui::{self, CentralPanel, Frame, Key, Margin, Modifiers, Panel};
use egui_commonmark::CommonMarkCache;
use tokio::{runtime::Runtime, task::JoinHandle};
use uuid::Uuid;

use crate::{
    app::{Approval, Emitter, Env, Event, health},
    domain::{chat::Conversation, settings::Settings},
    infra::{paths, store, vpn},
    tools::{checkpoint, webindex::WebIndex},
};

const NOTICE_TTL: Duration = Duration::from_secs(8);
const CHECKPOINT_MAX_AGE: Duration = Duration::from_hours(30 * 24);

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Omega")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([980.0, 640.0])
            .with_icon(Arc::new(theme::icon())),
        centered: true,
        ..Default::default()
    };
    eframe::run_native("Omega", options, Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))))
}

/// Streaming state of a running turn.
#[derive(Default)]
struct Live {
    text: String,
    reasoning: String,
    handle: Option<JoinHandle<()>>,
}

pub struct App {
    /// `Option` only so `Drop` can shut it down without waiting on background tasks.
    rt: Option<Runtime>,
    rx: mpsc::Receiver<Event>,
    emit: Emitter,
    web: Arc<WebIndex>,
    settings: Settings,
    env: Arc<Env>,
    convs: Vec<Conversation>,
    current: Option<Uuid>,
    live: HashMap<Uuid, Live>,
    approvals: Vec<Approval>,
    errors: HashMap<Uuid, String>,
    health: Vec<health::Health>,
    vpn: Option<vpn::VpnInfo>,
    vpn_task: Option<JoinHandle<()>>,
    draft: String,
    /// Message being edited: (conversation, message index).
    editing: Option<(Uuid, usize)>,
    search: String,
    /// Conversation whose delete button was clicked once (awaiting confirmation).
    delete_armed: Option<(Uuid, Instant)>,
    settings_draft: Option<Settings>,
    show_panel: bool,
    focus_composer: bool,
    notice: Option<(String, Instant)>,
    md: CommonMarkCache,
    /// Rendered (width, height, message timestamp) per (conversation, index) for off-screen skipping.
    heights: HashMap<(Uuid, usize), (f32, f32, i64)>,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        theme::install(ctx);
        let _ = paths::ensure();
        let (tx, rx) = mpsc::channel();
        let wake_ctx = ctx.clone();
        let emit = Emitter::new(tx, Arc::new(move || wake_ctx.request_repaint()));
        let web = Arc::new(WebIndex::open(&paths::web_cache()));
        let settings = store::load_settings();
        let (env, notice) = build_env(&settings, &web);
        let convs = store::load_conversations();
        let mut app = Self {
            rt: Some(Runtime::new().expect("tokio runtime")),
            rx,
            emit,
            web,
            settings,
            env,
            current: convs.first().map(|c| c.id),
            convs,
            live: HashMap::new(),
            approvals: Vec::new(),
            errors: HashMap::new(),
            health: Vec::new(),
            vpn: None,
            vpn_task: None,
            draft: String::new(),
            editing: None,
            search: String::new(),
            delete_armed: None,
            settings_draft: None,
            show_panel: true,
            focus_composer: true,
            notice: None,
            md: CommonMarkCache::default(),
            heights: HashMap::new(),
        };
        if let Some(n) = notice {
            app.notify(n);
        }
        if !app.env.llm.configured() || app.env.jev.is_none() {
            app.settings_draft = Some(app.settings.clone());
        }
        app.restart_background();
        app.spawn(async { tokio::task::spawn_blocking(|| checkpoint::prune(CHECKPOINT_MAX_AGE)).await });
        app
    }

    fn spawn<T: Send + 'static>(&self, task: impl Future<Output = T> + Send + 'static) -> JoinHandle<T> {
        self.rt.as_ref().expect("runtime alive until drop").spawn(task)
    }

    fn conv(&self, id: Uuid) -> Option<&Conversation> {
        self.convs.iter().find(|c| c.id == id)
    }

    fn conv_mut(&mut self, id: Uuid) -> Option<&mut Conversation> {
        self.convs.iter_mut().find(|c| c.id == id)
    }

    fn current_conv(&self) -> Option<&Conversation> {
        self.current.and_then(|id| self.conv(id))
    }

    fn busy(&self) -> bool {
        self.current.is_some_and(|id| self.live.contains_key(&id))
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), Instant::now()));
    }

    /// Current notice, expired ones dropped.
    fn notice(&mut self) -> Option<&str> {
        if self.notice.as_ref().is_some_and(|(_, at)| at.elapsed() > NOTICE_TTL) {
            self.notice = None;
        }
        self.notice.as_ref().map(|(text, _)| text.as_str())
    }

    fn save(&mut self, id: Uuid) {
        if let Some(conv) = self.conv(id)
            && let Err(e) = store::save_conversation(conv)
        {
            self.notify(format!("Sauvegarde impossible : {e:#}"));
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::N)) {
            self.new_chat();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if self.editing.is_some() {
                self.cancel_edit();
            } else if let Some(id) = self.current.filter(|_| self.busy()) {
                self.stop(id);
            }
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

fn build_env(settings: &Settings, web: &Arc<WebIndex>) -> (Arc<Env>, Option<String>) {
    match Env::new(settings.clone(), web.clone()) {
        Ok(env) => (Arc::new(env), None),
        Err(e) => {
            let fallback = Settings { proxy: String::new(), ..settings.clone() };
            let env = Env::new(fallback, web.clone()).expect("client HTTP sans proxy");
            (Arc::new(env), Some(format!("Proxy ignoré : {e:#}")))
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.pump();
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);
        if self.notice.is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        Panel::left("sidebar")
            .exact_size(272.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(theme::SIDEBAR).inner_margin(Margin::symmetric(14, 0)))
            .show(ui, |ui| sidebar::show(self, ui));
        if self.show_panel {
            Panel::right("tools")
                .exact_size(292.0)
                .resizable(false)
                .show_separator_line(false)
                .frame(Frame::new().fill(theme::SIDEBAR).inner_margin(Margin::symmetric(14, 0)))
                .show(ui, |ui| panel::show(self, ui));
        }
        Panel::bottom("composer")
            .show_separator_line(false)
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::symmetric(24, 10)))
            .show(ui, |ui| composer::show(self, ui));
        CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::symmetric(24, 0)))
            .show(ui, |ui| chat::show(self, ui));
        settings::show(self, &ctx);
    }
}
