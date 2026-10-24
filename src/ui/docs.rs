//! Text files joined to a prompt (button or drag-and-drop): their content travels inline with the message.

use std::path::Path;

use eframe::egui::Context;

/// Files one prompt may carry.
pub const MAX_DOCS: usize = 8;
/// Bytes read per file: beyond, the prompt would eat the context window.
const MAX_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doc {
    pub name: String,
    pub text: String,
}

/// Reads a UTF-8 text file; binaries and oversized files are refused with a readable reason.
pub fn from_path(path: &Path) -> Result<Doc, String> {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let size = std::fs::metadata(path).map_err(|e| format!("{name} : {e}"))?.len();
    if size > MAX_BYTES {
        return Err(format!("{name} : fichier trop lourd ({} Ko max)", MAX_BYTES / 1024));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name} : {e}"))?;
    from_bytes(name, bytes)
}

fn from_bytes(name: String, bytes: Vec<u8>) -> Result<Doc, String> {
    if bytes.contains(&0) {
        return Err(format!("{name} : fichier binaire non pris en charge (texte ou image uniquement)"));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("{name} : encodage non UTF-8"))?;
    Ok(Doc { name, text })
}

/// Non-image files dropped on the window this frame.
pub fn dropped(ctx: &Context, is_image: impl Fn(&Path) -> bool) -> Vec<Result<Doc, String>> {
    ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .filter(|f| !is_image(f.path()))
            // Path-less drops (from a browser) are images or nothing worth inlining: attach handles the images.
            .filter(|f| f.path().is_file())
            .map(|f| from_path(f.path()))
            .collect()
    })
}

/// The prompt followed by each file in a fence long enough not to be closed by the file's own backticks.
pub fn inline(prompt: &str, docs: &[Doc]) -> String {
    let mut out = prompt.to_owned();
    for doc in docs {
        let longest = doc.text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
        let fence = "`".repeat(longest.max(2) + 1);
        let lang = Path::new(&doc.name).extension().and_then(|e| e.to_str()).unwrap_or("");
        out.push_str(&format!("\n\nFichier joint `{}` :\n{fence}{lang}\n{}\n{fence}", doc.name, doc.text.trim_end()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_and_non_utf8_are_refused() {
        assert!(from_bytes("a.bin".into(), vec![1, 0, 2]).unwrap_err().contains("binaire"));
        assert!(from_bytes("a.txt".into(), vec![0xff, 0xfe]).unwrap_err().contains("UTF-8"));
        assert_eq!(from_bytes("a.rs".into(), b"fn x() {}".to_vec()).unwrap().text, "fn x() {}");
    }

    #[test]
    fn inline_fence_outlasts_backticks_in_the_file() {
        let doc = Doc { name: "notes.md".into(), text: "```rust\nx\n```\n".into() };
        let out = inline("Résume", &[doc]);
        assert!(out.starts_with("Résume\n\nFichier joint `notes.md` :\n````md\n```rust"));
        assert!(out.ends_with("```\n````"));
    }

    #[test]
    fn no_docs_keeps_prompt() {
        assert_eq!(inline("salut", &[]), "salut");
    }
}
