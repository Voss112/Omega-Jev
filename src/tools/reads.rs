//! Re-reads of an unchanged file range are answered with a stub: the full text is already on the wire.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Mutex, PoisonError},
};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::guard;
use crate::domain::{
    chat::{Message, Role},
    tool::ToolKind,
};

/// (normalised path, start, end) as the model asked for it.
pub type ReadKey = (String, Option<usize>, Option<usize>);

const STUB: &str = "[unchanged since your earlier read_file of";
/// Coarse filesystem clocks: a write this close to the read counts as a change.
const MARGIN_SECS: i64 = 2;

#[derive(Default)]
pub struct Reads {
    seen: Mutex<HashMap<ReadKey, DateTime<Utc>>>,
}

pub fn key(path: &str, start: Option<usize>, end: Option<usize>) -> ReadKey {
    let path = path.trim().replace('\\', "/");
    (path.strip_prefix("./").unwrap_or(&path).to_owned(), start, end)
}

impl Reads {
    /// Successful reads still visible to the model (pass the messages after any summary).
    pub fn from_history(messages: &[Message]) -> Self {
        let results: HashMap<&str, &Message> = messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .filter_map(|m| Some((m.tool_call_id.as_deref()?, m)))
            .collect();
        let mut seen = HashMap::new();
        for call in messages.iter().filter(|m| m.role == Role::Assistant).flat_map(|m| &m.tool_calls) {
            if ToolKind::parse(&call.name) != Some(ToolKind::ReadFile) {
                continue;
            }
            let Some(result) = results.get(call.id.as_str()) else { continue };
            if result.content.starts_with("ERROR:") || result.content.starts_with(STUB) {
                continue;
            }
            let args: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
            let Some(path) = args["path"].as_str() else { continue };
            let num = |k: &str| args[k].as_u64().map(|n| n as usize);
            seen.insert(key(path, num("start"), num("end")), result.at);
        }
        Self { seen: Mutex::new(seen) }
    }

    /// The stub when this exact range was read and the file has not been written since.
    pub fn unchanged(&self, root: &Path, key: &ReadKey) -> Option<String> {
        let at = *self.seen.lock().unwrap_or_else(PoisonError::into_inner).get(key)?;
        let path = guard::resolve_read(root, &key.0).ok()?;
        let modified: DateTime<Utc> = std::fs::metadata(path).and_then(|m| m.modified()).ok()?.into();
        (modified < at - chrono::Duration::seconds(MARGIN_SECS)).then(|| {
            let range = match key {
                (_, None, None) => String::new(),
                (_, s, e) => format!(" {}-{}", s.map_or(String::new(), |n| n.to_string()), e.map_or(String::new(), |n| n.to_string())),
            };
            format!("{STUB} {}{range}; reuse that output]", key.0)
        })
    }

    pub fn record(&self, key: ReadKey) {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).insert(key, Utc::now());
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::domain::chat::ToolCall;

    fn workspace(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("omega-reads-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = std::fs::File::create(dir.join("a.rs")).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(3600)).unwrap();
        dir
    }

    fn history(args: &str, output: &str) -> Vec<Message> {
        let call = ToolCall { id: "c1".into(), name: "read_file".into(), arguments: args.into() };
        vec![Message::assistant(String::new(), String::new(), vec![call]), Message::tool("c1".into(), output.into())]
    }

    #[test]
    fn an_unchanged_range_is_not_resent() {
        let root = workspace("same");
        let reads = Reads::from_history(&history(r#"{"path":"a.rs","start":1,"end":9}"#, "    1  fn main() {}"));
        let stub = reads.unchanged(&root, &key(".\\a.rs", Some(1), Some(9)));
        assert_eq!(stub.as_deref(), Some("[unchanged since your earlier read_file of a.rs 1-9; reuse that output]"));
    }

    #[test]
    fn a_modified_file_is_read_again() {
        let root = workspace("modified");
        let reads = Reads::from_history(&history(r#"{"path":"a.rs"}"#, "    1  fn main() {}"));
        std::fs::File::options().write(true).open(root.join("a.rs")).unwrap().set_modified(SystemTime::now()).unwrap();
        assert_eq!(reads.unchanged(&root, &key("a.rs", None, None)), None);
    }

    #[test]
    fn an_errored_read_does_not_count() {
        let root = workspace("error");
        let reads = Reads::from_history(&history(r#"{"path":"a.rs"}"#, "ERROR: cannot read a.rs"));
        assert_eq!(reads.unchanged(&root, &key("a.rs", None, None)), None);
    }

    #[test]
    fn another_range_is_read() {
        let root = workspace("range");
        let reads = Reads::from_history(&history(r#"{"path":"a.rs","start":1,"end":9}"#, "    1  fn main() {}"));
        assert_eq!(reads.unchanged(&root, &key("a.rs", Some(10), None)), None);
    }

    #[test]
    fn a_read_recorded_this_turn_counts_once_the_file_is_old_enough() {
        let root = workspace("record");
        let reads = Reads::default();
        reads.record(key("a.rs", None, None));
        assert!(reads.unchanged(&root, &key("a.rs", None, None)).is_some());
    }
}
