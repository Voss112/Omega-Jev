//! Per-provider model ids: curated suggestions, live account list, free-form override.

use super::engine::Engine;

/// Curated suggestion. `id` travels verbatim as the API `model` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    pub id: &'static str,
    pub name: &'static str,
}

/// One selectable model for the pickers: the live list first, curated fallback, current always first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub name: String,
}

impl Choice {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into() }
    }

    /// Dropdown entry: `Claude Opus 5.5 · claude-opus-5-5`, or just the id when there is no nicer name.
    pub fn label(&self) -> String {
        if self.name == self.id { self.id.clone() } else { format!("{} · {}", self.name, self.id) }
    }
}

const CLAUDE: &[Model] = &[
    Model { id: "claude-opus-5-5", name: "Claude Opus 5.5" },
    Model { id: "claude-sonnet-5-5", name: "Claude Sonnet 5.5" },
    Model { id: "claude-haiku-5-5", name: "Claude Haiku 5.5" },
    Model { id: "claude-fable-5-1", name: "Claude Fable 5.1" },
    Model { id: "claude-opus-4-5", name: "Claude Opus 4.5" },
    Model { id: "claude-sonnet-4-5", name: "Claude Sonnet 4.5" },
    Model { id: "claude-haiku-4-5", name: "Claude Haiku 4.5" },
];
const DEEPSEEK: &[Model] = &[Model { id: "deepseek-flash", name: "DeepSeek V4.1 Flash" }];

/// Offline fallback for `engine`; the account list fetched at runtime takes precedence.
pub fn suggested(engine: Engine) -> &'static [Model] {
    match engine {
        Engine::DeepSeek => DEEPSEEK,
        Engine::Claude => CLAUDE,
    }
}

/// Human name of `id`, or the raw id for a model the catalogue does not know yet.
pub fn name(engine: Engine, id: &str) -> String {
    suggested(engine).iter().find(|m| m.id == id).map_or_else(|| id.to_owned(), |m| m.name.to_owned())
}

/// Picker entries: the current model first (even when unknown), then the live list, then the suggestions.
pub fn choices(engine: Engine, current: &str, fetched: &[Choice]) -> Vec<Choice> {
    let mut out: Vec<Choice> = Vec::new();
    let current = current.trim();
    if !current.is_empty() {
        add(&mut out, current, &name(engine, current));
    }
    for c in fetched {
        let id = c.id.trim();
        if !id.is_empty() {
            add(&mut out, id, if c.name.trim().is_empty() { id } else { c.name.trim() });
        }
    }
    for m in suggested(engine) {
        add(&mut out, m.id, m.name);
    }
    out
}

fn add(out: &mut Vec<Choice>, id: &str, name: &str) {
    if !out.iter().any(|c| c.id == id) {
        out.push(Choice::new(id, name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_starts_with_a_suggestion() {
        for engine in Engine::ALL {
            assert!(!suggested(engine).is_empty(), "{engine:?}");
        }
        assert_eq!(name(Engine::Claude, "claude-opus-5-5"), "Claude Opus 5.5");
        assert_eq!(name(Engine::Claude, "claude-inconnu"), "claude-inconnu");
    }

    #[test]
    fn the_current_model_comes_first_and_is_never_duplicated() {
        let fetched = [Choice::new("claude-opus-5-5", "Claude Opus 5.5"), Choice::new("claude-x", "")];
        let choices = choices(Engine::Claude, "claude-opus-5-5", &fetched);
        assert_eq!(choices.first().map(|c| c.id.as_str()), Some("claude-opus-5-5"));
        assert_eq!(choices.iter().filter(|c| c.id == "claude-opus-5-5").count(), 1);
        assert!(choices.iter().any(|c| c.id == "claude-x" && c.name == "claude-x"), "un nom vide retombe sur l'id");
        assert!(choices.iter().any(|c| c.id == "claude-haiku-4-5"), "les suggestions restent offertes");
    }

    #[test]
    fn an_unknown_current_model_survives_the_merge() {
        let choices = choices(Engine::Claude, "claude-opus-9", &[]);
        assert_eq!(choices.first().map(|c| c.label()), Some("claude-opus-9".to_owned()));
    }
}
