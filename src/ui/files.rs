//! Explorer de fichiers : arbre à gauche, onglets au-dessus de la discussion, visionneuse en lecture seule.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use eframe::egui::{
    Align, CornerRadius, Frame, Label, Layout, Margin, RichText, ScrollArea, Stroke, TextWrapMode, Ui,
};

use super::{
    App,
    theme::{ACCENT, BORDER, CARD, CODE_BG, FAINT, MUTED, RAISED, RED, SOFT, TEXT, AMBER},
    widgets,
};

/// A file bigger than this is shown truncated: the viewer is a reader, not an editor.
const MAX_READ: u64 = 512 * 1024;
const MAX_ENTRIES: usize = 400;
const SKIP: &[&str] = &[
    ".git", "target", "node_modules", ".venv", "venv", "dist", "build", "__pycache__", ".idea", ".vscode", ".next", "coverage",
];

#[derive(Default)]
pub struct Files {
    pub show: bool,
    pub root: Option<PathBuf>,
    cache: HashMap<PathBuf, Vec<Entry>>,
    expanded: HashSet<PathBuf>,
    pub open: Vec<OpenFile>,
    pub active: Option<usize>,
    error: Option<String>,
}

pub struct Entry {
    name: String,
    dir: bool,
}

pub struct OpenFile {
    pub path: PathBuf,
    text: String,
    numbered: String,
    truncated: bool,
}

impl Files {
    /// Drops cached directory listings: the agent may have just written files.
    pub fn invalidate(&mut self) {
        self.cache.clear();
    }

    fn entries(&mut self, dir: &Path) -> &[Entry] {
        self.cache.entry(dir.to_owned()).or_insert_with(|| read_dir(dir))
    }

    fn open_file(&mut self, path: &Path, root: &Path) {
        if let Some(i) = self.open.iter().position(|f| f.path == path) {
            self.active = Some(i);
            return;
        }
        match read_text(path) {
            Ok((text, truncated)) => {
                let numbered = number(&text);
                self.open.push(OpenFile { path: path.to_owned(), text, numbered, truncated });
                self.active = Some(self.open.len() - 1);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{} : {e}", short(path, root))),
        }
    }

    fn close(&mut self, index: usize) {
        if index < self.open.len() {
            self.open.remove(index);
        }
        self.active = match self.active {
            Some(a) if a == index => (index < self.open.len()).then_some(index),
            Some(a) if a > index => Some(a - 1),
            other => other.filter(|a| *a < self.open.len()),
        };
    }
}

/// Left panel: workspace tree, dirs first, hidden folders skipped.
pub fn panel(app: &mut App, ui: &mut Ui) {
    ui.add_space(14.0);
    let root = app.env.workspace();
    app.files.root = Some(root.clone());
    ui.horizontal(|ui| {
        ui.label(RichText::new("Fichiers").font(super::theme::bold(15.0)).color(TEXT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::ghost(ui, "⟳").on_hover_text("Relire le dossier").clicked() {
                app.files.invalidate();
            }
        });
    });
    ui.add_space(2.0);
    ui.label(RichText::new(short(&root, &root).replace('\\', " › ")).size(10.5).color(FAINT)).on_hover_text(root.display().to_string());
    ui.add_space(6.0);
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        tree(app, ui, root.clone(), 0);
        if let Some(e) = &app.files.error {
            ui.add_space(6.0);
            ui.label(RichText::new(e.clone()).size(11.5).color(RED));
        }
        ui.add_space(12.0);
    });
}

fn tree(app: &mut App, ui: &mut Ui, dir: PathBuf, depth: usize) {
    if depth > 10 {
        return;
    }
    let root = app.files.root.clone().unwrap_or_else(|| dir.clone());
    let entries = app.files.entries(&dir);
    for i in 0..entries.len() {
        let (name, is_dir) = {
            let e = &app.files.entries(&dir)[i];
            (e.name.clone(), e.dir)
        };
        let path = dir.join(&name);
        let indent = depth as f32 * 12.0;
        if is_dir {
            let expanded = app.files.expanded.contains(&path);
            ui.horizontal(|ui| {
                ui.add_space(indent);
                let arrow = if expanded { "▾" } else { "▸" };
                let resp = ui.add(
                    eframe::egui::Button::new(RichText::new(format!("{arrow} 📁 {name}")).size(12.5).color(MUTED)).frame(false),
                );
                if resp.clicked() {
                    if expanded {
                        app.files.expanded.remove(&path);
                    } else {
                        app.files.expanded.insert(path.clone());
                    }
                }
            });
            if app.files.expanded.contains(&path) {
                tree(app, ui, path, depth + 1);
            }
        } else {
            let selected = app.files.active.and_then(|a| app.files.open.get(a)).is_some_and(|f| f.path == path);
            ui.horizontal(|ui| {
                ui.add_space(indent);
                let color = if selected { ACCENT } else { SOFT };
                let resp = ui.add(
                    eframe::egui::Button::new(RichText::new(format!("{} {}", icon(&name), name)).size(12.5).color(color))
                        .frame(false),
                );
                if resp.clicked() {
                    app.files.open_file(&path, &root);
                }
            });
        }
    }
}

/// Tab strip above the conversation; returns `true` when a file is displayed.
pub fn tabs(app: &mut App, ui: &mut Ui) -> bool {
    if app.files.open.is_empty() {
        return false;
    }
    ui.add_space(6.0);
    let mut close = None;
    let mut select = None;
    ScrollArea::horizontal().id_salt("file-tabs").max_height(30.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            for (i, file) in app.files.open.iter().enumerate() {
                let active = app.files.active == Some(i);
                let name = file.path.file_name().map_or_else(|| file.path.display().to_string(), |n| n.to_string_lossy().into_owned());
                let color = if active { TEXT } else { MUTED };
                let frame = Frame::new()
                    .fill(if active { RAISED } else { CARD })
                    .stroke(Stroke::new(1.0, if active { ACCENT.gamma_multiply(0.6) } else { BORDER }))
                    .corner_radius(CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 })
                    .inner_margin(Margin { left: 10, right: 4, top: 4, bottom: 4 });
                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.add(eframe::egui::Button::new(RichText::new(name).size(12.0).color(color)).frame(false)).clicked() {
                            select = Some(i);
                        }
                        if ui
                            .add(eframe::egui::Button::new(RichText::new("✕").size(11.0).color(FAINT)).frame(false))
                            .on_hover_text("Fermer l'onglet")
                            .clicked()
                        {
                            close = Some(i);
                        }
                    });
                });
            }
            if ui.add(eframe::egui::Button::new(RichText::new("⌫ Tout fermer").size(11.0).color(FAINT)).frame(false)).clicked() {
                close = Some(usize::MAX);
            }
        });
    });
    match (close, select) {
        (Some(usize::MAX), _) => {
            app.files.open.clear();
            app.files.active = None;
        }
        (Some(i), _) => app.files.close(i),
        (None, Some(i)) => app.files.active = Some(i),
        (None, None) => {}
    }
    ui.add_space(2.0);
    let rect = ui.max_rect();
    ui.painter().hline(rect.x_range(), ui.cursor().top(), Stroke::new(1.0, BORDER));
    ui.add_space(6.0);
    app.files.active.is_some()
}

/// Read-only viewer with line numbers.
pub fn viewer(app: &mut App, ui: &mut Ui) {
    let Some(index) = app.files.active else { return };
    let Some(file) = app.files.open.get(index) else {
        app.files.active = None;
        return;
    };
    let root = app.files.root.clone().unwrap_or_default();
    let rel = short(&file.path, &root);
    let mut close = false;
    let mut copy = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(rel).font(super::theme::bold(14.0)).color(TEXT)).on_hover_text(file.path.display().to_string());
        if file.truncated {
            widgets::chip(ui, "tronqué à 512 Ko", AMBER);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::ghost(ui, "✕ Fermer").clicked() {
                close = true;
            }
            if widgets::ghost(ui, "⧉ Copier").on_hover_text("Copier le contenu").clicked() {
                copy = true;
            }
        });
    });
    ui.add_space(6.0);
    let text = file.numbered.clone();
    Frame::new()
        .fill(CODE_BG)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height((ui.available_height() - 12.0).max(120.0));
            ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                ui.add(Label::new(RichText::new(text).monospace().size(12.0).color(TEXT)).wrap_mode(TextWrapMode::Extend));
            });
        });
    if copy {
        let raw = app.files.open[index].text.clone();
        ui.ctx().copy_text(raw);
        app.notify("Contenu copié");
    }
    if close {
        app.files.close(index);
    }
}

fn read_dir(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut entries: Vec<Entry> = rd
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            !SKIP.contains(&name.as_str())
        })
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let dir = e.file_type().ok()?.is_dir();
            Some(Entry { name, dir })
        })
        .collect();
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    entries.truncate(MAX_ENTRIES);
    entries
}

fn read_text(path: &Path) -> std::io::Result<(String, bool)> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut buf = Vec::with_capacity(len.min(MAX_READ) as usize);
    BufReader::new(file).take(MAX_READ).read_to_end(&mut buf)?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if text.contains('\u{0}') {
        text = "Fichier binaire : aperçu indisponible.".to_owned();
    }
    Ok((text, len > MAX_READ))
}

fn number(text: &str) -> String {
    let lines = text.lines().count().max(1);
    let width = lines.to_string().len();
    let mut out = String::with_capacity(text.len() + lines * (width + 3));
    for (i, line) in text.lines().enumerate() {
        out.push_str(&format!("{:>width$} │ {line}\n", i + 1, width = width));
    }
    out
}

/// Path relative to the workspace when possible, with `/` separators.
pub fn short(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

fn icon(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" => "🦀",
        "toml" | "yaml" | "yml" | "json" => "⚙",
        "md" | "txt" => "📄",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" => "🖼",
        "ps1" | "sh" | "bat" => "⌘",
        _ => "▫",
    }
}

/// Click a file in the tree: keeps the explorer open and shows the tab.
pub fn toggle(app: &mut App) {
    app.files.show = !app.files.show;
}
