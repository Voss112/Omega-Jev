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
}

impl Provider {
    pub const ALL: [Provider; 1] = [Self::Claude];

    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
        }
    }

    pub fn engine(self) -> crate::domain::engine::Engine {
        match self {
            Self::Claude => crate::domain::engine::Engine::Claude,
        }
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
}

/// Tokens never reach a log, a panic message or a debug print.
impl std::fmt::Debug for Cred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cred")
            .field("access", &"<scellé>")
            .field("refresh", &"<scellé>")
            .field("expires_at", &self.expires_at)
            .field("account", &self.account)
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
        assert_eq!(Provider::ALL, [Provider::Claude]);
        assert_eq!(Provider::Claude.engine(), crate::domain::engine::Engine::Claude);
        assert_eq!(Provider::Claude.id(), "claude");
    }
}
