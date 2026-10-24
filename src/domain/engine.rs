use serde::{Deserialize, Serialize};

/// Wire protocol spoken by a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire {
    OpenAi,
    Anthropic,
}

/// Reasoning budget. `Off` sends no reasoning parameter at all. Ordered from least to most reasoning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Off,
    /// Default: shortest thinking, cheapest.
    #[default]
    Low,
    Medium,
    High,
    Max,
}

impl Effort {
    pub const ALL: [Effort; 5] = [Self::Off, Self::Low, Self::Medium, Self::High, Self::Max];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Max => "Max",
        }
    }

    /// OpenAI-compatible `reasoning_effort`; `None` omits the field.
    pub fn openai(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::Max => Some("max"),
        }
    }

    /// Claude `output_config.effort` (adaptive-thinking models). `Off` asks for the shortest thinking:
    /// the newest models cannot turn it off.
    pub fn claude_effort(self) -> &'static str {
        match self {
            Self::Off | Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }

    /// Claude extended-thinking budget, in tokens (models older than the 4.6 generation).
    pub fn claude_budget(self) -> Option<u32> {
        match self {
            Self::Off => None,
            Self::Low => Some(1024),
            Self::Medium => Some(4096),
            Self::High => Some(8192),
            Self::Max => Some(16384),
        }
    }
}

/// System Two engines. Effort is chosen by the user, never by the router.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// Default. Also takes in settings written when Kimi K3.1 Pro (retired) was selected.
    #[default]
    #[serde(alias = "deepseek", alias = "kimi")]
    DeepSeek,
    /// Also takes in settings written when Gemini (retired) was selected.
    #[serde(alias = "gemini")]
    Claude,
}

impl Engine {
    pub const ALL: [Engine; 2] = [Self::DeepSeek, Self::Claude];

    pub fn label(self) -> &'static str {
        match self {
            Self::DeepSeek => "DeepSeek V4.1 Flash",
            Self::Claude => "Claude",
        }
    }

    pub fn wire(self) -> Wire {
        match self {
            Self::DeepSeek => Wire::OpenAi,
            Self::Claude => Wire::Anthropic,
        }
    }

    pub fn default_effort(self) -> Effort {
        match self {
            Self::DeepSeek => Effort::Max,
            Self::Claude => Effort::Medium,
        }
    }

    /// Fallback environment variable for the API key.
    pub fn key_var(self) -> &'static str {
        match self {
            Self::DeepSeek => "DEEPSEEK_API_KEY",
            Self::Claude => "ANTHROPIC_API_KEY",
        }
    }

    /// Providers that can run on a consumer subscription instead of an API key.
    pub fn subscription_capable(self) -> bool {
        self == Self::Claude
    }

    /// Multimodal input (screenshots). DeepSeek text models do not accept images.
    pub fn vision(self) -> bool {
        self == Self::Claude
    }
}

/// One effort per engine, so switching engine never loses the user's choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Efforts {
    pub deepseek: Effort,
    pub claude: Effort,
}

impl Default for Efforts {
    fn default() -> Self {
        Self { deepseek: Engine::DeepSeek.default_effort(), claude: Engine::Claude.default_effort() }
    }
}

impl Efforts {
    pub fn get(self, engine: Engine) -> Effort {
        match engine {
            Engine::DeepSeek => self.deepseek,
            Engine::Claude => self.claude,
        }
    }

    pub fn set(&mut self, engine: Engine, effort: Effort) {
        match engine {
            Engine::DeepSeek => self.deepseek = effort,
            Engine::Claude => self.claude = effort,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_labels_cover_both_engines() {
        assert_eq!(Engine::ALL.len(), 2);
        assert_eq!(Engine::default(), Engine::DeepSeek);
        assert_eq!(Engine::default_effort(Engine::DeepSeek), Effort::Max);
        assert_eq!(Engine::wire(Engine::Claude), Wire::Anthropic);
        assert_eq!(Engine::wire(Engine::DeepSeek), Wire::OpenAi);
        assert!(Engine::Claude.subscription_capable() && !Engine::DeepSeek.subscription_capable());
    }

    #[test]
    fn effort_maps_to_each_provider() {
        assert_eq!(Effort::Off.openai(), None);
        assert_eq!((Effort::Low.claude_budget(), Effort::Max.claude_budget()), (Some(1024), Some(16384)));
        assert_eq!((Effort::Off.claude_effort(), Effort::Max.claude_effort()), ("low", "max"));
    }

    #[test]
    fn efforts_keep_one_choice_per_engine() {
        let mut e = Efforts::default();
        e.set(Engine::Claude, Effort::High);
        assert_eq!((e.get(Engine::Claude), e.get(Engine::DeepSeek)), (Effort::High, Effort::Max));
    }
}
