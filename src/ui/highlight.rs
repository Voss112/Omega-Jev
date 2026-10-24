//! Syntax colours for the file editor (syntect, the grammars egui_commonmark already ships).

use std::sync::LazyLock;

use eframe::egui::{Color32, FontId, TextFormat, text::LayoutJob};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

/// Past this size a file is shown in plain monospace: colouring it on every keystroke would stall the frame.
pub const MAX_BYTES: usize = 200 * 1024;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME: LazyLock<Theme> = LazyLock::new(|| {
    let mut set = ThemeSet::load_defaults();
    set.themes.remove("base16-ocean.dark").unwrap_or_default()
});

/// Grammar name for a file extension (TypeScript reads well with the JavaScript grammar).
fn syntax_for(ext: &str) -> &'static syntect::parsing::SyntaxReference {
    let ext = match ext {
        "ts" | "tsx" | "mts" | "cts" | "jsx" | "mjs" | "cjs" => "js",
        "toml" | "ini" | "cfg" => "ini",
        "kts" => "kt",
        other => other,
    };
    SYNTAXES.find_syntax_by_extension(ext).unwrap_or_else(|| SYNTAXES.find_syntax_plain_text())
}

/// The coloured layout of `text`; plain monospace (`fallback` colour) when too large or unknown.
pub fn job(text: &str, ext: &str, font: &FontId, fallback: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let plain = |job: &mut LayoutJob, piece: &str| job.append(piece, 0.0, TextFormat::simple(font.clone(), fallback));
    if text.len() > MAX_BYTES {
        plain(&mut job, text);
        return job;
    }
    let mut lines = HighlightLines::new(syntax_for(ext), &THEME);
    for line in LinesWithEndings::from(text) {
        match lines.highlight_line(line, &SYNTAXES) {
            Ok(spans) => {
                for (style, piece) in spans {
                    let fg = style.foreground;
                    let mut format = TextFormat::simple(font.clone(), Color32::from_rgb(fg.r, fg.g, fg.b));
                    format.italics = style.font_style.contains(FontStyle::ITALIC);
                    if style.font_style.contains(FontStyle::UNDERLINE) {
                        format.underline = eframe::egui::Stroke::new(1.0, format.color);
                    }
                    job.append(piece, 0.0, format);
                }
            }
            Err(_) => plain(&mut job, line),
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_code_gets_several_colours_and_keeps_its_text() {
        let code = "fn main() {\n    let x = \"texte\"; // note\n}\n";
        let job = job(code, "rs", &FontId::monospace(12.0), Color32::WHITE);
        assert_eq!(job.text, code);
        let colours: std::collections::HashSet<_> = job.sections.iter().map(|s| s.format.color).collect();
        assert!(colours.len() >= 3, "{colours:?}");
    }

    #[test]
    fn unknown_or_huge_files_stay_plain() {
        let big = "x".repeat(MAX_BYTES + 1);
        assert_eq!(job(&big, "rs", &FontId::monospace(12.0), Color32::WHITE).sections.len(), 1);
        let text = job("abc", "zzz-inconnu", &FontId::monospace(12.0), Color32::WHITE);
        assert_eq!(text.text, "abc");
    }
}
