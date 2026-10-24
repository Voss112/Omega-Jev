use serde::{Deserialize, Serialize};

/// How a provider is authenticated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    ApiKey,
    #[default]
    Subscription,
}

/// Providers that accept a consumer subscription through OAuth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Gemini,
    /// Not an engine: git and `gh` credentials for the agent's shell.
    GitHub,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Self::Claude, Self::Gemini, Self::GitHub];
    /// Providers that run a System Two engine.
    pub const ENGINES: [Provider; 2] = [Self::Claude, Self::Gemini];

    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::GitHub => "github",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Gemini => "Gemini",
            Self::GitHub => "GitHub",
        }
    }

    pub fn engine(self) -> Option<crate::domain::engine::Engine> {
        match self {
            Self::Claude => Some(crate::domain::engine::Engine::Claude),
            Self::Gemini => Some(crate::domain::engine::Engine::Gemini),
            Self::GitHub => None,
        }
    }

    pub fn of(engine: crate::domain::engine::Engine) -> Option<Self> {
        Self::ENGINES.into_iter().find(|p| p.engine() == Some(engine))
    }
}

/// OAuth credentials of one subscription. `access`/`refresh` are sealed at rest.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Cred {
    pub access: String,
    pub refresh: String,
    /// Unix seconds; 0 = unknown (refreshed before use).
    pub expires_at: i64,
    /// Account label shown in the UI (email, plan or tier).
    pub account: String,
    /// Gemini Code Assist project id (empty for other providers).
    pub project: String,
}

/// Tokens never reach a log, a panic message or a debug print.
impl std::fmt::Debug for Cred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cred")
            .field("access", &"<scellé>")
            .field("refresh", &"<scellé>")
            .field("expires_at", &self.expires_at)
            .field("account", &self.account)
            .field("project", &self.project)
            .finish()
    }
}

/// One usage window of a subscription plan (best-effort, hidden when unavailable).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Window {
    pub label: String,
    /// 0.0..=1.0 of the plan allowance already used.
    pub fraction: f32,
    pub resets: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_tokens() {
        let cred = Cred { access: "at-secret".into(), refresh: "rt-secret".into(), ..Cred::default() };
        let text = format!("{cred:?}");
        assert!(!text.contains("at-secret") && !text.contains("rt-secret"), "{text}");
        assert!(text.contains("scellé"));
    }

    #[test]
    fn subscription_providers_map_to_their_engine() {
        use crate::domain::engine::Engine;
        assert_eq!(Provider::Claude.engine(), Some(Engine::Claude));
        assert_eq!(Provider::Gemini.engine(), Some(Engine::Gemini));
        assert_eq!(Provider::GitHub.engine(), None);
        assert_eq!(Provider::of(Engine::Gemini), Some(Provider::Gemini));
        assert_eq!(Provider::of(Engine::DeepSeek), None);
        assert_eq!(Provider::Claude.id(), "claude");
    }
}
