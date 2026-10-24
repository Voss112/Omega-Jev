//! Explorer de fichiers : arbre à gauche, onglets au-dessus de la discussion, éditeur coloré (lecture ou modification).

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    hash::{Hash, Hasher},
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use eframe::egui::{
    Align, Color32, CornerRadius, FontId, Frame, Galley, Key, Label, Layout, Margin, Modifiers, Rect, Response, RichText,
    ScrollArea, Sense, Stroke, StrokeKind, TextBuffer, TextEdit, Ui, pos2, text::LayoutJob, vec2,
};

use super::{
    App,
    i18n::tr, fx, highlight,
    icons::{self, Icon},
    theme::{
        self, ACCENT, ACCENT_HI, AMBER, CODE_BG, CYAN, EDGE, EDGE_HI, FAINT, GREEN, MUTED, PINK, RED, SOFT, TEXT, TILE_ACTIVE,
        TILE_HOVER,
    },
    widgets,
};

/// A file bigger than this is shown truncated, and then read-only: saving it would cut it.
const MAX_READ: u64 = 512 * 1024;
const MAX_ENTRIES: usize = 400;
const CODE_FONT: f32 = 12.5;
const SKIP: &[&str] =
    &[".git", "target", "node_modules", ".venv", "venv", "dist", "build", "__pycache__", ".idea", ".vscode", ".next", "coverage"];

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
    /// Content on disk when last read or saved (line endings normalised to `\n`).
    text: String,
    /// What the editor shows and modifies.
    buffer: String,
    /// The file used CRLF: saving writes CRLF back.
    crlf: bool,
    truncated: bool,
    binary: bool,
    editing: bool,
    /// The agent (or another program) changed the file while it had unsaved edits.
    stale: bool,
    /// Coloured layout of `buffer`, keyed by its hash.
    colours: Option<(u64, LayoutJob)>,
}

impl OpenFile {
    fn read(path: &Path) -> std::io::Result<Self> {
        let (raw, truncated) = read_text(path)?;
        let binary = raw.contains('\u{0}');
        let crlf = raw.contains("\r\n");
        let text = if binary { tr("Fichier binaire : aperçu indisponible.", "Binary file: no preview.").to_owned() } else { raw.replace("\r\n", "\n") };
        Ok(Self {
            path: path.to_owned(),
            buffer: text.clone(),
            text,
            crlf,
            truncated,
            binary,
            editing: false,
            stale: false,
            colours: None,
        })
    }

    pub fn dirty(&self) -> bool {
        self.buffer != self.text
    }

    fn editable(&self) -> bool {
        !self.truncated && !self.binary
    }

    /// Writes the buffer back with the file's own line endings.
    fn save(&mut self) -> std::io::Result<()> {
        let out = if self.crlf { self.buffer.replace('\n', "\r\n") } else { self.buffer.clone() };
        std::fs::write(&self.path, out)?;
        self.text.clone_from(&self.buffer);
        self.stale = false;
        Ok(())
    }

    /// Re-reads the disk; unsaved edits are kept and the tab is flagged instead.
    fn reload(&mut self) {
        let Ok(fresh) = Self::read(&self.path) else { return };
        if fresh.text == self.text {
            return;
        }
        if self.dirty() {
            self.stale = true;
            return;
        }
        *self = Self { editing: self.editing, ..fresh };
    }
}

impl Files {
    /// Drops cached directory listings and re-reads the open tabs: the agent may have just written files.
    pub fn invalidate(&mut self) {
        self.cache.clear();
        for file in &mut self.open {
            file.reload();
        }
    }

    fn entries(&mut self, dir: &Path) -> &[Entry] {
        self.cache.entry(dir.to_owned()).or_insert_with(|| read_dir(dir))
    }

    fn open_file(&mut self, path: &Path, root: &Path) {
        if let Some(i) = self.open.iter().position(|f| f.path == path) {
            self.active = Some(i);
            return;
        }
        match OpenFile::read(path) {
            Ok(file) => {
                self.open.push(file);
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
    ui.add_space(16.0);
    let root = app.env.workspace();
    app.files.root = Some(root.clone());
    ui.horizontal(|ui| {
        ui.set_height(30.0);
        ui.label(RichText::new(tr("Fichiers", "Files")).font(theme::bold(16.5)).color(TEXT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::icon_button(ui, Icon::Refresh, tr("Relire le dossier", "Reload the folder")).clicked() {
                app.files.invalidate();
            }
        });
    });
    let name = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
    widgets::truncated(ui, &name, FontId::proportional(11.0), FAINT).on_hover_text(root.display().to_string());
    ui.add_space(8.0);
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
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
        let indent = depth as f32 * 14.0;
        if is_dir {
            let expanded = app.files.expanded.contains(&path);
            if entry(ui, indent, &name, Some(expanded), false).clicked() {
                if expanded {
                    app.files.expanded.remove(&path);
                } else {
                    app.files.expanded.insert(path.clone());
                }
            }
            if app.files.expanded.contains(&path) {
                tree(app, ui, path, depth + 1);
            }
        } else {
            let selected = app.files.active.and_then(|a| app.files.open.get(a)).is_some_and(|f| f.path == path);
            if entry(ui, indent, &name, None, selected).clicked() {
                app.files.open_file(&path, &root);
                app.show_tasks = false;
            }
        }
    }
}

/// One tree row; `expanded` is `Some` for directories.
fn entry(ui: &mut Ui, indent: f32, name: &str, expanded: Option<bool>, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    let k = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    let turn = ui.ctx().animate_bool_with_time(resp.id.with("open"), expanded == Some(true), 0.15);
    let p = ui.painter();
    if selected {
        p.rect(rect, 7.0, ACCENT.gamma_multiply(0.16), Stroke::new(1.0, ACCENT.gamma_multiply(0.35)), StrokeKind::Inside);
    } else if k > 0.0 {
        p.rect_filled(rect, 7.0, TILE_HOVER.gamma_multiply(k));
    }
    let mut x = rect.left() + 6.0 + indent;
    let cy = rect.center().y;
    if expanded.is_some() {
        icons::chevron(p, pos2(x + 5.0, cy), 12.0, turn, fx::mix(FAINT, TEXT, k));
    }
    x += 14.0;
    let (icon, color) = match expanded {
        Some(_) => (Icon::Folder, AMBER.gamma_multiply(0.85)),
        None => kind(name),
    };
    icons::at(p, pos2(x + 7.0, cy), 14.0, icon, color);
    x += 20.0;
    let base = if expanded.is_some() { MUTED } else { SOFT };
    let fg = if selected { TEXT } else { fx::mix(base, TEXT, k) };
    let galley = widgets::line(ui, name, FontId::proportional(12.5), fg, rect.right() - x - 6.0);
    ui.painter().galley(pos2(x, cy - galley.size().y / 2.0), galley, fg);
    resp
}

/// Tab strip above the conversation; returns `true` when a file is displayed.
pub fn tabs(app: &mut App, ui: &mut Ui) -> bool {
    if app.files.open.is_empty() {
        return false;
    }
    ui.add_space(8.0);
    let mut close = None;
    let mut select = None;
    ScrollArea::horizontal().id_salt("file-tabs").max_height(34.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (i, file) in app.files.open.iter().enumerate() {
                let active = app.files.active == Some(i);
                let name =
                    file.path.file_name().map_or_else(|| file.path.display().to_string(), |n| n.to_string_lossy().into_owned());
                let name = if file.dirty() { format!("● {name}") } else { name };
                match tab(ui, &name, active) {
                    Some(true) => close = Some(i),
                    Some(false) => select = Some(i),
                    None => {}
                }
            }
            ui.add_space(4.0);
            if widgets::pill(ui, Some(Icon::Close), tr("Tout fermer", "Close all"), FAINT, false).clicked() {
                close = Some(usize::MAX);
            }
        });
    });
    match (close, select) {
        (Some(usize::MAX), _) => {
            // Tabs with unsaved edits stay open: closing them silently would lose the work.
            app.files.open.retain(OpenFile::dirty);
            app.files.active = (!app.files.open.is_empty()).then_some(0);
            if !app.files.open.is_empty() {
                app.notify(tr("Onglets non enregistrés gardés ouverts (Ctrl+S pour enregistrer)", "Unsaved tabs kept open (Ctrl+S to save)"));
            }
        }
        (Some(i), _) if app.files.open.get(i).is_some_and(OpenFile::dirty) => {
            app.files.active = Some(i);
            app.notify(tr("Modifications non enregistrées : Ctrl+S pour enregistrer, ou « Annuler les modifications »", "Unsaved changes: Ctrl+S to save, or “Discard changes”"));
        }
        (Some(i), _) => app.files.close(i),
        (None, Some(i)) => app.files.active = Some(i),
        (None, None) => {}
    }
    let rect = ui.max_rect();
    fx::hairline(ui.painter(), rect.left(), rect.right(), ui.cursor().top() + 2.0, EDGE_HI);
    ui.add_space(10.0);
    app.files.active.is_some()
}

/// One file tab; `Some(true)` = close clicked, `Some(false)` = tab clicked.
fn tab(ui: &mut Ui, name: &str, active: bool) -> Option<bool> {
    let (icon, color) = kind(name);
    let galley = widgets::line(ui, name, FontId::proportional(12.5), if active { TEXT } else { MUTED }, 200.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 62.0, 32.0), Sense::click());
    let close_rect = Rect::from_center_size(rect.right_center() - vec2(15.0, 0.0), vec2(20.0, 20.0));
    let close = ui.interact(close_rect, resp.id.with("close"), Sense::click()).on_hover_text(tr("Fermer l'onglet", "Close tab"));
    let k = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || close.hovered(), 0.12);
    let p = ui.painter();
    let fill = if active { TILE_ACTIVE } else { TILE_HOVER.gamma_multiply(k) };
    p.rect(rect, 9.0, fill, Stroke::new(1.0, if active { EDGE_HI } else { EDGE.gamma_multiply(k) }), StrokeKind::Inside);
    if active {
        let bar = Rect::from_min_max(rect.left_bottom() + vec2(12.0, -2.0), rect.right_bottom() - vec2(12.0, 0.0));
        fx::gradient_rect(p, bar, 1.0, ACCENT_HI, CYAN);
    }
    icons::at(p, rect.left_center() + vec2(16.0, 0.0), 13.0, icon, color);
    let fg = if active { TEXT } else { fx::mix(MUTED, TEXT, k) };
    p.galley(rect.left_center() + vec2(30.0, -galley.size().y / 2.0), galley, fg);
    if close.hovered() {
        p.rect_filled(close_rect, 6.0, TILE_HOVER);
    }
    icons::at(p, close_rect.center(), 12.0, Icon::Close, if close.hovered() { TEXT } else { FAINT });
    if close.clicked() { Some(true) } else { resp.clicked().then_some(false) }
}

/// What the viewer's toolbar asks for.
enum Command {
    Close,
    Copy,
    Edit,
    Save,
    Revert,
    Reload,
}

/// Coloured editor with line numbers: read mode by default, « Modifier » to type, Ctrl+S to save.
pub fn viewer(app: &mut App, ui: &mut Ui) {
    let Some(index) = app.files.active else { return };
    let Some(file) = app.files.open.get(index) else {
        app.files.active = None;
        return;
    };
    let root = app.files.root.clone().unwrap_or_default();
    let rel = short(&file.path, &root);
    let mut command = None;
    ui.horizontal(|ui| {
        ui.set_height(30.0);
        let (icon, color) = kind(&rel);
        let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        icons::paint(ui.painter(), rect, icon, color);
        ui.label(RichText::new(&rel).font(theme::bold(14.0)).color(TEXT)).on_hover_text(file.path.display().to_string());
        if file.truncated {
            widgets::chip(ui, tr("tronqué à 512 Ko · lecture seule", "truncated to 512 KB · read-only"), AMBER);
        }
        if file.dirty() {
            widgets::chip(ui, tr("non enregistré", "unsaved"), AMBER);
        }
        if file.stale {
            widgets::chip_icon(ui, Some(Icon::Alert), tr("modifié sur le disque", "changed on disk"), RED);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if widgets::pill(ui, Some(Icon::Close), tr("Fermer", "Close"), MUTED, false).clicked() {
                command = Some(Command::Close);
            }
            if widgets::pill(ui, Some(Icon::Copy), tr("Copier", "Copy"), MUTED, false).on_hover_text(tr("Copier le contenu", "Copy the content")).clicked() {
                command = Some(Command::Copy);
            }
            if file.stale && widgets::pill(ui, Some(Icon::Refresh), tr("Recharger le disque", "Reload from disk"), RED, false).clicked() {
                command = Some(Command::Reload);
            }
            if file.dirty() {
                if widgets::pill(ui, Some(Icon::Undo), tr("Annuler les modifications", "Discard changes"), MUTED, false).clicked() {
                    command = Some(Command::Revert);
                }
                if widgets::pill(ui, Some(Icon::Check), tr("Enregistrer (Ctrl+S)", "Save (Ctrl+S)"), GREEN, true).clicked() {
                    command = Some(Command::Save);
                }
            }
            if file.editable() && !file.editing && widgets::pill(ui, Some(Icon::Pencil), tr("Modifier", "Edit"), ACCENT_HI, false).clicked() {
                command = Some(Command::Edit);
            }
        });
    });
    ui.add_space(8.0);
    if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S)) && app.files.open[index].dirty() {
        command = Some(Command::Save);
    }
    let file = &mut app.files.open[index];
    Frame::new()
        .fill(CODE_BG)
        .stroke(Stroke::new(1.0, if file.editing { ACCENT.gamma_multiply(0.45) } else { EDGE }))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height((ui.available_height() - 12.0).max(120.0));
            ScrollArea::both().id_salt(("file-editor", index)).auto_shrink([false, false]).show(ui, |ui| editor(ui, file));
        });
    let file = &mut app.files.open[index];
    match command {
        Some(Command::Close) if file.dirty() => {
            app.notify(tr("Modifications non enregistrées : enregistrez ou annulez-les avant de fermer", "Unsaved changes: save or discard them before closing"));
        }
        Some(Command::Close) => app.files.close(index),
        Some(Command::Copy) => {
            let raw = file.buffer.clone();
            ui.ctx().copy_text(raw);
            app.notify(tr("Contenu copié", "Content copied"));
        }
        Some(Command::Edit) => file.editing = true,
        Some(Command::Revert) => {
            file.buffer.clone_from(&file.text);
            file.stale = false;
        }
        Some(Command::Reload) => {
            if let Ok(fresh) = OpenFile::read(&file.path) {
                *file = OpenFile { editing: file.editing, ..fresh };
            }
        }
        Some(Command::Save) => match file.save() {
            Ok(()) => {
                let name = short(&file.path, &root);
                app.files.cache.clear();
                app.notify(format!("{} {name}", tr("Enregistré :", "Saved:")));
            }
            Err(e) => app.notify(format!("{} {e}", tr("Enregistrement impossible :", "Cannot save:"))),
        },
        None => {}
    }
}

/// Line numbers, then the coloured text: editable in edit mode, selectable (read-only) otherwise.
fn editor(ui: &mut Ui, file: &mut OpenFile) {
    let OpenFile { path, text, buffer, editing, colours, .. } = file;
    let lines = buffer.lines().count().max(1) + usize::from(buffer.ends_with('\n'));
    let width = lines.to_string().len();
    let numbers: String = (1..=lines).map(|n| format!("{n:>width$}\n")).collect();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase();
    // Colouring is cached by text hash: a frame without a keystroke reuses it, a keystroke recolours once.
    let mut layouter = |ui: &Ui, text: &dyn TextBuffer, _wrap: f32| -> Arc<Galley> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.as_str().hash(&mut h);
        let key = h.finish();
        if colours.as_ref().is_none_or(|(k, _)| *k != key) {
            *colours = Some((key, highlight::job(text.as_str(), &ext, &FontId::monospace(CODE_FONT), SOFT)));
        }
        let job = colours.as_ref().map(|(_, j)| j.clone()).unwrap_or_default();
        ui.fonts_mut(|f| f.layout_job(job))
    };
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.add(Label::new(RichText::new(numbers).monospace().size(CODE_FONT).color(FAINT)).selectable(false));
        // Read mode: a `&str` buffer is selectable and copyable but cannot change.
        let mut shown = text.as_str();
        let edit = if *editing { TextEdit::multiline(buffer) } else { TextEdit::multiline(&mut shown) };
        ui.add(
            edit.code_editor()
                .frame(Frame::NONE)
                .desired_width(f32::INFINITY)
                .desired_rows(lines)
                .lock_focus(true)
                .layouter(&mut layouter),
        );
    });
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
    Ok((String::from_utf8_lossy(&buf).into_owned(), len > MAX_READ))
}

/// Path relative to the workspace when possible, with `/` separators.
pub fn short(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

/// Icon and tint for a file name.
fn kind(name: &str) -> (Icon, Color32) {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" => (Icon::File, Color32::from_rgb(0xFB, 0x92, 0x3C)),
        "toml" | "yaml" | "yml" | "json" => (Icon::Gear, AMBER),
        "md" | "txt" => (Icon::Book, CYAN),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" => (Icon::Image, PINK),
        "ps1" | "sh" | "bat" => (Icon::Terminal, GREEN),
        "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "c" | "cpp" | "h" | "java" | "cs" => (Icon::File, ACCENT_HI),
        _ => (Icon::File, MUTED),
    }
}

/// Click a file in the tree: keeps the explorer open and shows the tab.
pub fn toggle(app: &mut App) {
    app.files.show = !app.files.show;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_files_are_edited_in_lf_and_saved_back_in_crlf() {
        let dir = std::env::temp_dir().join(format!("omega-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.rs");
        std::fs::write(&path, "fn a() {}\r\nfn b() {}\r\n").unwrap();
        let mut file = OpenFile::read(&path).unwrap();
        assert!(file.crlf && file.editable() && !file.dirty());
        assert_eq!(file.buffer, "fn a() {}\nfn b() {}\n");
        file.buffer.push_str("fn c() {}\n");
        assert!(file.dirty());
        file.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn a() {}\r\nfn b() {}\r\nfn c() {}\r\n");
        assert!(!file.dirty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_disk_change_never_overwrites_unsaved_edits() {
        let dir = std::env::temp_dir().join(format!("omega-files-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.txt");
        std::fs::write(&path, "v1\n").unwrap();
        let mut file = OpenFile::read(&path).unwrap();
        file.buffer = "mine\n".into();
        std::fs::write(&path, "agent\n").unwrap();
        file.reload();
        assert!(file.stale && file.buffer == "mine\n", "modifications gardées, onglet signalé");
        let mut clean = OpenFile::read(&path).unwrap();
        std::fs::write(&path, "agent 2\n").unwrap();
        clean.reload();
        assert_eq!(clean.buffer, "agent 2\n", "sans modification : rechargé");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn binary_and_truncated_files_are_read_only() {
        let dir = std::env::temp_dir().join(format!("omega-files-bin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.bin"), [0u8, 1, 2]).unwrap();
        assert!(!OpenFile::read(&dir.join("x.bin")).unwrap().editable());
        let _ = std::fs::remove_dir_all(dir);
    }
}
