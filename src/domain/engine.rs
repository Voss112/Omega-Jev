use serde::{Deserialize, Serialize};

/// System Two engines. Each has a fixed reasoning effort chosen by the user, never by the router.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// Kimi K3.1 Pro, effort `low` (always thinks; low keeps it short and cheap).
    #[default]
    Kimi,
    /// DeepSeek V4.1 Flash 1017, effort `max`.
    DeepSeek,
}

impl Engine {
    pub const ALL: [Engine; 2] = [Self::Kimi, Self::DeepSeek];

    pub fn label(self) -> &'static str {
        match self {
            Self::Kimi => "Kimi K3.1 Pro",
            Self::DeepSeek => "DeepSeek V4.1 Flash",
        }
    }

    pub fn effort(self) -> &'static str {
        match self {
            Self::Kimi => "low",
            Self::DeepSeek => "max",
        }
    }

    /// Fallback environment variable for the API key.
    pub fn key_var(self) -> &'static str {
        match self {
            Self::Kimi => "MOONSHOT_API_KEY",
            Self::DeepSeek => "DEEPSEEK_API_KEY",
        }
    }
}
