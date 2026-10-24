//! Markdown with readable tables. egui_commonmark lays a table out in an `egui::Grid` whose cells only get last
//! frame's column width (40 px at first): with wrapping on, text wraps to that width and the column never grows.
//! Top-level tables are drawn here with explicit column widths; everything else stays with egui_commonmark.

use eframe::egui::{Align, CornerRadius, Frame, Layout, Margin, Rect, Shape, Stroke, TextStyle, Ui, pos2, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use super::theme::{EDGE, TEXT, rgba};

/// Narrowest column: a few characters stay readable.
const MIN_COL: f32 = 56.0;
const PAD_X: i8 = 8;
const PAD_Y: i8 = 5;

#[derive(Debug, PartialEq)]
pub enum Block<'a> {
    Text(&'a str),
    Table { header: Vec<&'a str>, rows: Vec<Vec<&'a str>> },
}

/// Markdown split into plain runs and top-level tables (cell sources kept as markdown).
pub fn blocks(text: &str) -> Vec<Block<'_>> {
    if !text.contains('|') {
        return vec![Block::Text(text)];
    }
    let mut out = Vec::new();
    let (mut last, mut depth) = (0, 0usize);
    let mut table: Option<(Vec<&str>, Vec<Vec<&str>>)> = None;
    let mut in_head = false;
    for (event, range) in Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter() {
        match event {
            Event::Start(Tag::Table(_)) if depth == 0 => {
                push_text(&mut out, &text[last..range.start]);
                table = Some((Vec::new(), Vec::new()));
            }
            Event::Start(Tag::TableHead) => in_head = true,
            Event::End(TagEnd::TableHead) => in_head = false,
            Event::Start(Tag::TableRow) => {
                if let Some((_, rows)) = table.as_mut() {
                    rows.push(Vec::new());
                }
            }
            Event::Start(Tag::TableCell) => {
                if let Some((header, rows)) = table.as_mut() {
                    let cell = text[range.clone()].trim().trim_matches('|').trim();
                    match (in_head, rows.last_mut()) {
                        (true, _) => header.push(cell),
                        (false, Some(row)) => row.push(cell),
                        (false, None) => {}
                    }
                }
            }
            Event::End(TagEnd::Table) if depth == 1 => {
                if let Some((header, rows)) = table.take() {
                    out.push(Block::Table { header, rows });
                }
                last = range.end;
            }
            _ => {}
        }
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    push_text(&mut out, &text[last..]);
    out
}

fn push_text<'a>(out: &mut Vec<Block<'a>>, text: &'a str) {
    if !text.trim().is_empty() {
        out.push(Block::Text(text));
    }
}

/// Column widths within `avail`: everything at its natural width when it fits, otherwise the columns that fit
/// an even share keep their width and the wider ones split the rest in proportion to their natural width.
pub fn fit(natural: &[f32], avail: f32) -> Vec<f32> {
    let natural: Vec<f32> = natural.iter().map(|w| w.max(MIN_COL)).collect();
    if natural.iter().sum::<f32>() <= avail {
        return natural;
    }
    let mut widths = natural.clone();
    let mut wide: Vec<usize> = (0..natural.len()).collect();
    let mut left = avail;
    loop {
        let share = left / wide.len().max(1) as f32;
        let (narrow, rest): (Vec<usize>, Vec<usize>) = wide.iter().partition(|&&i| natural[i] <= share);
        if narrow.is_empty() || rest.is_empty() {
            let total: f32 = rest.iter().map(|&i| natural[i]).sum();
            for &i in &rest {
                widths[i] = (left * natural[i] / total).max(MIN_COL);
            }
            return widths;
        }
        left -= narrow.iter().map(|&i| natural[i]).sum::<f32>();
        wide = rest;
    }
}

/// Drop-in for `CommonMarkViewer::new().show`.
pub fn show(ui: &mut Ui, md: &mut CommonMarkCache, text: &str) {
    for (k, block) in blocks(text).iter().enumerate() {
        ui.push_id(("md", k), |ui| match block {
            Block::Text(t) => {
                CommonMarkViewer::new().show(ui, md, t);
            }
            Block::Table { header, rows } => table(ui, md, header, rows),
        });
    }
}

fn table(ui: &mut Ui, md: &mut CommonMarkCache, header: &[&str], rows: &[Vec<&str>]) {
    let cols = rows.iter().map(Vec::len).chain([header.len()]).max().unwrap_or(0);
    if cols == 0 {
        return;
    }
    let font = TextStyle::Body.resolve(ui.style());
    let pad = f32::from(PAD_X) * 2.0 + 2.0;
    let natural: Vec<f32> = (0..cols)
        .map(|c| {
            [header].into_iter().chain(rows.iter().map(Vec::as_slice))
                .filter_map(|r| r.get(c))
                .map(|cell| ui.painter().layout_no_wrap(plain(cell), font.clone(), TEXT).size().x)
                .fold(0.0, f32::max)
                + pad
        })
        .collect();
    let widths = fit(&natural, ui.available_width() - 2.0);
    ui.add_space(4.0);
    let frame = Frame::new().stroke(Stroke::new(1.0, EDGE)).corner_radius(CornerRadius::same(6)).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let head = ui.painter().add(Shape::Noop);
        let rect = row(ui, md, header, &widths, true);
        ui.painter().set(head, Shape::rect_filled(rect, CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, rgba(255, 255, 255, 10)));
        for (k, cells) in rows.iter().enumerate() {
            let top = ui.cursor().top();
            ui.painter().hline(rect.x_range(), top, Stroke::new(1.0, EDGE));
            ui.push_id(k, |ui| row(ui, md, cells, &widths, false));
        }
    });
    let r = frame.response.rect;
    let mut x = r.left() + 1.0;
    for w in &widths[..widths.len() - 1] {
        x += w;
        ui.painter().vline(x, r.y_range(), Stroke::new(1.0, EDGE));
    }
    ui.add_space(6.0);
}

/// One row, every cell at its column width; returns the row's rect.
fn row(ui: &mut Ui, md: &mut CommonMarkCache, cells: &[&str], widths: &[f32], header: bool) -> Rect {
    let left = ui.cursor().left();
    let r = ui
        .horizontal_top(|ui| {
            for (c, &w) in widths.iter().enumerate() {
                let cell = cells.get(c).copied().unwrap_or_default();
                ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(w);
                    Frame::new().inner_margin(Margin::symmetric(PAD_X, PAD_Y)).show(ui, |ui| {
                        ui.set_width((w - f32::from(PAD_X) * 2.0).max(1.0));
                        if header {
                            ui.visuals_mut().override_text_color = Some(TEXT);
                        }
                        ui.push_id(c, |ui| CommonMarkViewer::new().show(ui, md, cell));
                    });
                });
            }
        })
        .response
        .rect;
    Rect::from_min_max(pos2(left, r.top()), pos2(left + widths.iter().sum::<f32>(), r.bottom()))
}

/// Cell text without the markers that take no width.
fn plain(cell: &str) -> String {
    cell.replace("**", "").replace("__", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_top_level_table_is_split_from_the_text_around_it() {
        let md = "Intro **x**.\n\n| A | B |\n|---|---|\n| **un** : long | `c\\|d` |\n| 2 |\n\nFin.";
        let b = blocks(md);
        assert_eq!(b.len(), 3, "{b:?}");
        assert_eq!(b[0], Block::Text("Intro **x**.\n\n"));
        assert_eq!(b[1], Block::Table { header: vec!["A", "B"], rows: vec![vec!["**un** : long", "`c\\|d`"], vec!["2", ""]] });
        assert_eq!(b[2], Block::Text("\nFin."));
    }

    #[test]
    fn pipes_in_code_or_without_a_table_stay_text() {
        let fenced = "```\n| A | B |\n|---|---|\n| 1 | 2 |\n```";
        assert_eq!(blocks(fenced), vec![Block::Text(fenced)]);
        assert_eq!(blocks("a | b"), vec![Block::Text("a | b")]);
        let nested = "- item\n\n  | A |\n  |---|\n  | 1 |\n";
        assert_eq!(blocks(nested), vec![Block::Text(nested)], "un tableau dans une liste reste à egui_commonmark");
    }

    #[test]
    fn columns_fit_the_width_and_long_ones_share_it() {
        assert_eq!(fit(&[100.0, 200.0], 500.0), vec![100.0, 200.0]);
        let w = fit(&[150.0, 1200.0, 600.0], 700.0);
        assert_eq!(w[0], 150.0, "la colonne courte garde sa largeur");
        assert!((w.iter().sum::<f32>() - 700.0).abs() < 0.01, "{w:?}");
        assert!((w[1] / w[2] - 2.0).abs() < 0.01, "partage proportionnel {w:?}");
        assert!(fit(&[10.0, 5000.0], 300.0).iter().all(|&x| x >= MIN_COL));
    }

    /// The table of the bug report, laid out headless in a 700 px column with wrapping forced (as `chat::column` does).
    fn laid_out(render: fn(&mut Ui, &mut CommonMarkCache, &str)) -> eframe::egui::Rect {
        use eframe::egui::{Context, RawInput, TextWrapMode};
        const REPORT: &str = "| Changement | Résultat sur l'API réelle |\n|---|---|\n\
            | **Effacement des anciens résultats d'outils en plein tour** : au-delà du budget de compaction, les gros \
            résultats anciens sont remplacés par une ligne de résumé ; les 6 derniers restent intacts | Requête de 20,2 Ko \
            ramenée à 11,5 Ko. Claude 5.5 l'accepte même avec sa réflexion signée. Le texte reste visible dans l'interface |\n\
            | **Règles de sortie communes** : pas de commentaire entre les appels d'outils | Texte visible −4 %, −31 % et −41 % |";
        let ctx = Context::default();
        let mut md = CommonMarkCache::default();
        let mut rect = eframe::egui::Rect::NOTHING;
        for _ in 0..4 {
            let input = RawInput { screen_rect: Some(eframe::egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(700.0, 5000.0))), ..RawInput::default() };
            let mut out = ctx.run_ui(input, |ui| {
                ui.style_mut().wrap_mode = Some(TextWrapMode::Wrap);
                rect = ui.scope(|ui| render(ui, &mut md, REPORT)).response.rect;
            });
            out.textures_delta.clear();
        }
        rect
    }

    #[test]
    fn the_reported_table_wraps_at_readable_column_widths() {
        let old = laid_out(|ui, md, text| {
            CommonMarkViewer::new().show(ui, md, text);
        });
        let new = laid_out(show);
        println!("tableau du rapport : avant {:.0}×{:.0} px → après {:.0}×{:.0} px", old.width(), old.height(), new.width(), new.height());
        assert!(new.width() <= 700.5, "le tableau tient dans la colonne ({})", new.width());
        assert!(new.height() < 260.0, "tableau compact : {} px de haut", new.height());
        assert!(old.height() > 2.0 * new.height(), "le rendu d'origine reproduit le bug : {} px vs {} px", old.height(), new.height());
    }
}
