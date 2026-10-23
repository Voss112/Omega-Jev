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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Gemini,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Self::Claude, Self::Gemini];

    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Gemini => "gemini",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Gemini => "Gemini",
        }
    }

    pub fn engine(self) -> crate::domain::engine::Engine {
        match self {
            Self::Claude => crate::domain::engine::Engine::Claude,
            Self::Gemini => crate::domain::engine::Engine::Gemini,
        }
    }
}

/// OAuth credentials of one subscription. `access`/`refresh` are sealed at rest.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Cred {
    pub access: String,
    pub refresh: String,
    /// Unix seconds; 0 = unknown (refreshed before use).
    pub expires_at: i64,
    /// Account label shown in the UI (email, plan or tier).
    pub account: String,
    /// Gemini Code Assist project id (empty for Claude).
    pub project: String,
}

/// One usage window of a subscription plan (best-effort, hidden when unavailable).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Window {
    pub label: String,
    /// 0.0..=1.0 of the plan allowance already used.
    pub fraction: f32,
    pub resets: Option<String>,
}
