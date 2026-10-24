//! egui front-end: owns conversations, bridges UI ↔ tokio runtime through events.

mod actions;
mod attach;
mod cards;
mod chat;
mod composer;
mod dictation;
mod docs;
mod events;
mod extras;
mod files;
mod highlight;
mod fx;
mod icons;
mod login;
mod markdown;
mod memory;
mod panel;
mod pace;
mod plans;
mod queue;
mod savings;
mod scale;
mod schedule;
mod settings;
mod sidebar;
mod tasks;
mod theme;
mod tray;
mod update;
mod widgets;
mod window;

use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use eframe::egui::{self, Align2, Area, CentralPanel, Frame, Id, Key, Margin, Modifiers, Order, Panel, Sense, Ui, vec2};
use egui_commonmark::CommonMarkCache;
use tokio::{runtime::Runtime, task::JoinHandle};
use uuid::Uuid;

use crate::{
    app::{inbox::Inbox,Approval, Emitter, Env, Event, health},
    domain::{
        auth::{Provider, Window},
        chat::Conversation,
        engine::Engine,
        models::Choice,
        settings::Settings,
    },
    infra::{paths, store, vpn},
    tools::{checkpoint, webindex::WebIndex},
};

const NOTICE_TTL: Duration = Duration::from_secs(8);
const SIDEBAR_W: f32 = 284.0;
const FILES_W: f32 = 262.0;
const TOOLS_W: f32 = 312.0;
const CHECKPOINT_MAX_AGE: Duration = Duration::from_hours(30 * 24);

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Omega")
            .with_inner_size([1400.0, 880.0])
            .with_min_inner_size([960.0, 600.0])
            .with_decorations(false)
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
    /// Engine and model of this run (a scheduled job runs with its own), stamped on a stopped answer.
    engine: Engine,
    model: String,
    /// Effort the running step uses (Jev may lower it), known once the first step starts.
    effort: Option<crate::domain::engine::Effort>,
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
    /// Last token-economy benchmark, and whether one is running.
    bench: Option<Vec<health::Health>>,
    bench_running: bool,
    vpn: Option<vpn::VpnInfo>,
    vpn_task: Option<JoinHandle<()>>,
    draft: String,
    /// Message being edited: (conversation, message index).
    editing: Option<(Uuid, usize)>,
    search: String,
    /// Conversation whose delete button was clicked once (awaiting confirmation).
    delete_armed: Option<(Uuid, Instant)>,
    settings_draft: Option<Settings>,
    /// In-flight OAuth dance, if any.
    login: Option<crate::app::login::Login>,
    /// Background half of a login (loopback listener, device polling), aborted on cancel.
    login_task: Option<JoinHandle<()>>,
    /// GitHub token typed by the user (`ghp_…`, `github_pat_…`).
    github_draft: String,
    /// The Antigravity installer is running.
    agy_installing: bool,
    /// Long-lived Claude token typed by the user (`claude setup-token`).
    token_draft: String,
    /// Last fetched plan usage per subscription.
    plan_usage: HashMap<Provider, (Vec<Window>, Instant)>,
    /// Last quota request per provider (success or not): paces the 30 s refresh.
    usage_polled: HashMap<Provider, Instant>,
    /// Connected account per subscription, read once (decrypting credentials every frame is wasteful).
    accounts: HashMap<Provider, String>,
    scaler: scale::Scaler,
    show_panel: bool,
    focus_composer: bool,
    notice: Option<(String, Instant)>,
    md: CommonMarkCache,
    /// Rendered (width, height, message timestamp) per (conversation, index) for off-screen skipping.
    heights: HashMap<(Uuid, usize), (f32, f32, i64)>,
    /// Explorer, open tabs and the read-only viewer.
    files: files::Files,
    /// Prompts programmés (toujours en bypass).
    jobs: Vec<crate::domain::schedule::Job>,
    /// Tasks tab shown in place of the conversation.
    show_tasks: bool,
    /// Task whose delete button was clicked once (awaiting confirmation).
    task_delete_armed: Option<(Uuid, Instant)>,
    schedule_draft: Option<schedule::Draft>,
    schedule_error: Option<String>,
    /// Screenshots waiting in the composer.
    shots: Vec<crate::domain::chat::Image>,
    /// Decoded thumbnails, keyed by image hash.
    tex: HashMap<u64, egui::TextureHandle>,
    /// Conversation the textures were last pruned for.
    tex_owner: Option<Uuid>,
    /// Last connection warm-up (throttled while typing).
    warmed: Option<Instant>,
    /// Live model list per provider, as returned by the account.
    catalog: HashMap<Engine, Vec<Choice>>,
    catalog_error: HashMap<Engine, String>,
    /// Free-form model id being typed in the composer.
    model_input: Option<String>,
    /// Voice dictation in progress.
    voice: Option<dictation::Voice>,
    /// Prompts typed while a turn runs, sent one by one when it ends.
    queue: HashMap<Uuid, std::collections::VecDeque<queue::Queued>>,
    /// Text files joined to the draft, inlined at send.
    docs: Vec<docs::Doc>,
    /// Memory notes shown in the panel; `None` → re-read from disk.
    memory_notes: Option<Vec<crate::infra::memory::Note>>,
    /// System tray icon (absent when disabled or unavailable).
    tray: Option<tray::Tray>,
    /// Settings changed: the tray icon is re-synced on the next frame (needs the egui context).
    tray_sync: bool,
    /// Update downloaded in the background, installed silently on exit.
    update: Option<std::path::PathBuf>,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        theme::install(ctx);
        let _ = paths::ensure();
        let (tx, rx) = mpsc::channel();
        let wake_ctx = ctx.clone();
        let emit = Emitter::new(tx, Arc::new(move || wake_ctx.request_repaint()));
        let web = Arc::new(WebIndex::open(&paths::web_cache()));
        let (settings, unreadable) = store::load_settings();
        let (env, notice) = build_env(&settings, &web, &Inbox::default());
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
            bench: None,
            bench_running: false,
            vpn: None,
            vpn_task: None,
            draft: String::new(),
            editing: None,
            search: String::new(),
            delete_armed: None,
            settings_draft: None,
            login: None,
            login_task: None,
            github_draft: String::new(),
            agy_installing: false,
            token_draft: String::new(),
            plan_usage: HashMap::new(),
            usage_polled: HashMap::new(),
            accounts: HashMap::new(),
            scaler: scale::Scaler::default(),
            show_panel: true,
            focus_composer: true,
            notice: None,
            md: CommonMarkCache::default(),
            heights: HashMap::new(),
            files: files::Files::default(),
            jobs: store::load_jobs(),
            show_tasks: false,
            task_delete_armed: None,
            schedule_draft: None,
            schedule_error: None,
            shots: Vec::new(),
            tex: HashMap::new(),
            tex_owner: None,
            warmed: None,
            catalog: HashMap::new(),
            catalog_error: HashMap::new(),
            model_input: None,
            voice: None,
            queue: HashMap::new(),
            docs: Vec::new(),
            memory_notes: None,
            tray: None,
            tray_sync: false,
            update: None,
        };
        if let Some(n) = unreadable.or(notice) {
            app.notify(n);
        }
        app.reload_accounts();
        // Gemini on a personal plan needs the Antigravity CLI: installed by default, then the sign-in console opens.
        let s = &app.settings;
        let wants_agy = s.gemini_auth == crate::domain::auth::AuthMode::Subscription
            && (s.engine == Engine::Gemini || (s.delegate && s.delegate_gemini));
        if wants_agy && crate::infra::llm::agy::binary().is_none() {
            app.install_agy(true);
        }
        // Quotas: first read on the first frame, then every 30 s (`poll_plan_usage`).
        // First launch, or nothing to run with. A missing Jev key is optional (local routing, chip in the top bar).
        if !app.env.llm.configured() || !paths::settings().exists() {
            app.settings_draft = Some(app.settings.clone());
        }
        app.refresh_health();
        app.restart_vpn();
        let engine = app.settings.engine;
        app.refresh_models(engine);
        app.spawn(async { tokio::task::spawn_blocking(|| checkpoint::prune(CHECKPOINT_MAX_AGE)).await });
        app.apply_system(ctx);
        app.start_updates();
        if tray::background_launch() && app.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
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

    /// GPU textures of other conversations are freed when the view switches.
    fn evict_textures(&mut self) {
        self.tex_owner = self.current;
        let keep = match self.current_conv() {
            Some(conv) => attach::image_keys(conv, &self.shots),
            None => self.shots.iter().map(attach::key).collect(),
        };
        self.tex.retain(|k, _| keep.contains(k));
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::N)) {
            self.new_chat();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::M)) {
            self.toggle_dictation(ctx);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if let Some(v) = &self.voice {
                v.stop();
            } else if self.editing.is_some() {
                self.cancel_edit();
            } else if let Some(id) = self.current.filter(|_| self.busy()) {
                self.stop(id);
            }
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Test watchers live outside the runtime: their process trees are killed before Omega exits.
        crate::tools::livetest::stop_all();
        self.apply_update();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

fn build_env(settings: &Settings, web: &Arc<WebIndex>, inbox: &Inbox) -> (Arc<Env>, Option<String>) {
    let (mut env, notice) = match Env::new(settings.clone(), web.clone()) {
        Ok(env) => (env, None),
        Err(e) => {
            let fallback = Settings { proxy: String::new(), ..settings.clone() };
            let env = Env::new(fallback, web.clone()).expect("client HTTP sans proxy");
            (env, Some(format!("Proxy ignoré : {e:#}")))
        }
    };
    env.inbox = inbox.clone();
    (Arc::new(env), notice)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.pump();
        let ctx = ui.ctx().clone();
        self.pump_tray(&ctx, frame);
        self.poll_plan_usage(&ctx);
        if std::mem::take(&mut self.tray_sync) {
            self.sync_tray(&ctx);
        }
        self.scaler.update(&ctx);
        let narrow = ctx.content_rect().width() < scale::NARROW_W;
        self.pump_dictation(&ctx);
        if self.tex_owner != self.current {
            self.evict_textures();
        }
        self.shortcuts(&ctx);
        fx::backdrop(&ctx, ctx.content_rect(), fx::now(&ctx));
        let activity = ctx.input(|i| pace::Activity {
            focused: i.viewport().focused.unwrap_or(true),
            minimized: i.viewport().minimized.unwrap_or(false),
            busy: !self.live.is_empty(),
            animating: self.login.is_some() || self.bench_running || !i.raw.hovered_files.is_empty(),
            timers: self.notice.is_some() || !self.jobs.is_empty(),
        });
        if let Some(delay) = pace::next_frame(activity) {
            ctx.request_repaint_after(delay);
        }

        Panel::top("titlebar")
            .exact_size(window::TITLE_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE)
            .show(ui, |ui| window::title_bar(self, ui));
        Panel::left("sidebar")
            .exact_size(SIDEBAR_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(column(theme::GAP, 0))
            .show(ui, |ui| {
                glass(ui);
                sidebar::show(self, ui);
            });
        // Narrow window: side columns fold away without forgetting the user's choice.
        let mut show_files = self.files.show && !narrow;
        Panel::left("files")
            .exact_size(FILES_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(column(theme::GAP, 0))
            .show_collapsible(ui, &mut show_files, |ui| {
                glass(ui);
                files::panel(self, ui);
            });
        if !narrow {
            self.files.show = show_files;
        }
        let mut show_tools = self.show_panel && !narrow;
        Panel::right("tools")
            .exact_size(TOOLS_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(column(0, theme::GAP))
            .show_collapsible(ui, &mut show_tools, |ui| {
                glass(ui);
                panel::show(self, ui);
            });
        if !narrow {
            self.show_panel = show_tools;
        }
        Panel::bottom("composer")
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().inner_margin(Margin { left: 22, right: 22, top: 4, bottom: 10 }))
            .show(ui, |ui| composer::show(self, ui));
        self.warm_up();
        let central = CentralPanel::default()
            .frame(Frame::new().inner_margin(Margin::symmetric(22, 0)))
            .show(ui, |ui| {
                if self.show_tasks {
                    tasks::view(self, ui);
                } else if files::tabs(self, ui) {
                    files::viewer(self, ui);
                } else {
                    chat::show(self, ui);
                }
            })
            .response
            .rect;
        settings::show(self, &ctx);
        login::show(self, &ctx);
        schedule::show(self, &ctx);
        self.toast(&ctx, central);
        window::resize_edges(&ctx);
        self.fire_due_jobs();
    }
}

/// Floating column: gap to the window edge on one side, content padding inside.
fn column(left: i8, right: i8) -> Frame {
    Frame::new().outer_margin(Margin { left, right, top: 0, bottom: theme::GAP }).inner_margin(Margin::symmetric(PAD, 0))
}

const PAD: i8 = 14;

/// Paints the frosted surface behind a column; call first inside the panel.
fn glass(ui: &Ui) {
    let rect = ui.max_rect().expand2(vec2(f32::from(PAD), 0.0));
    let shadow = ui.painter().with_clip_rect(ui.clip_rect().expand(40.0));
    fx::shadow(&shadow, rect, theme::PANEL_RADIUS, 110);
    fx::glass(ui.painter(), rect, theme::PANEL_RADIUS, theme::GLASS);
}

impl App {
    /// Notice toast: slides down under the title bar, fades out, click to dismiss.
    fn toast(&mut self, ctx: &egui::Context, over: egui::Rect) {
        let Some(text) = self.notice().map(str::to_owned) else { return };
        let age = self.notice.as_ref().map_or(0.0, |(_, at)| at.elapsed().as_secs_f32());
        let fade_in = egui::emath::easing::cubic_out((age / 0.3).min(1.0));
        let fade_out = ((NOTICE_TTL.as_secs_f32() - age) / 0.45).clamp(0.0, 1.0);
        let k = fade_in * fade_out;
        let dismissed = Area::new(Id::unique("toast"))
            .order(Order::Tooltip)
            .pivot(Align2::CENTER_TOP)
            .fixed_pos(egui::pos2(over.center().x, over.top() + 64.0 - 14.0 * (1.0 - k)))
            .show(ctx, |ui| {
                ui.multiply_opacity(k);
                let frame = Frame::new()
                    .fill(theme::GLASS_STRONG)
                    .stroke(egui::Stroke::new(1.0, theme::EDGE_HI))
                    .corner_radius(14)
                    .inner_margin(Margin { left: 12, right: 16, top: 9, bottom: 9 })
                    .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: egui::Color32::from_black_alpha(140) });
                frame
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                            icons::paint(ui.painter(), rect, icons::Icon::Sparkle, theme::ACCENT_HI);
                            ui.label(egui::RichText::new(text).size(13.0).color(theme::TEXT));
                        });
                    })
                    .response
                    .interact(Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
            })
            .inner;
        if dismissed {
            self.notice = None;
        }
    }
}
