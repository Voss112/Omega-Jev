use serde::{Deserialize, Serialize};

/// Wire protocol spoken by a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire {
    OpenAi,
    Anthropic,
    /// Gemini Code Assist (`v1internal`), subscription tokens only.
    Gemini,
    /// Gemini through the Antigravity CLI (`agy`, personal plans): text in, text out, no tools.
    Antigravity,
}

/// Reasoning budget. `Off` turns thinking off wherever the model allows it. Ordered from least to most reasoning.
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

    /// Gemini `thinkingConfig.thinkingBudget`; `-1` = dynamic.
    pub fn gemini_budget(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::Low => 1024,
            Self::Medium => 4096,
            Self::High => 8192,
            Self::Max => -1,
        }
    }
}

/// System Two engines. Effort is chosen by the user; Jev may only lower it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// Default.
    #[default]
    #[serde(alias = "deepseek")]
    DeepSeek,
    Claude,
    /// Google AI Pro/Ultra subscription (Code Assist) or a Gemini API key.
    Gemini,
    /// Moonshot API key (OpenAI-compatible endpoint).
    Kimi,
}

impl Engine {
    pub const ALL: [Engine; 4] = [Self::DeepSeek, Self::Claude, Self::Gemini, Self::Kimi];

    pub fn label(self) -> &'static str {
        match self {
            Self::DeepSeek => "DeepSeek V4.1 Flash",
            Self::Claude => "Claude",
            Self::Gemini => "Gemini",
            Self::Kimi => "Kimi K3.1 Pro",
        }
    }

    /// Native wire; a Gemini API key speaks the OpenAI-compatible endpoint instead (see `Llm::wire`).
    pub fn wire(self) -> Wire {
        match self {
            Self::DeepSeek | Self::Kimi => Wire::OpenAi,
            Self::Claude => Wire::Anthropic,
            Self::Gemini => Wire::Gemini,
        }
    }

    pub fn default_effort(self) -> Effort {
        match self {
            Self::DeepSeek => Effort::Max,
            Self::Kimi => Effort::High,
            Self::Claude | Self::Gemini => Effort::Medium,
        }
    }

    /// Fallback environment variable for the API key.
    pub fn key_var(self) -> &'static str {
        match self {
            Self::DeepSeek => "DEEPSEEK_API_KEY",
            Self::Claude => "ANTHROPIC_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Kimi => "MOONSHOT_API_KEY",
        }
    }

    /// Providers that can run on a consumer subscription instead of an API key.
    pub fn subscription_capable(self) -> bool {
        matches!(self, Self::Claude | Self::Gemini)
    }

    /// Multimodal input (screenshots). DeepSeek text models do not accept images.
    pub fn vision(self) -> bool {
        matches!(self, Self::Claude | Self::Gemini)
    }
}

/// One effort per engine, so switching engine never loses the user's choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Efforts {
    pub deepseek: Effort,
    pub claude: Effort,
    pub gemini: Effort,
    pub kimi: Effort,
}

impl Default for Efforts {
    fn default() -> Self {
        Self {
            deepseek: Engine::DeepSeek.default_effort(),
            claude: Engine::Claude.default_effort(),
            gemini: Engine::Gemini.default_effort(),
            kimi: Engine::Kimi.default_effort(),
        }
    }
}

impl Efforts {
    pub fn get(self, engine: Engine) -> Effort {
        match engine {
            Engine::DeepSeek => self.deepseek,
            Engine::Claude => self.claude,
            Engine::Gemini => self.gemini,
            Engine::Kimi => self.kimi,
        }
    }

    pub fn set(&mut self, engine: Engine, effort: Effort) {
        match engine {
            Engine::DeepSeek => self.deepseek = effort,
            Engine::Claude => self.claude = effort,
            Engine::Gemini => self.gemini = effort,
            Engine::Kimi => self.kimi = effort,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_labels_cover_every_engine() {
        assert_eq!(Engine::ALL.len(), 4);
        assert_eq!(Engine::default(), Engine::DeepSeek);
        assert_eq!(Engine::default_effort(Engine::DeepSeek), Effort::Max);
        assert_eq!(Engine::wire(Engine::Claude), Wire::Anthropic);
        assert_eq!(Engine::wire(Engine::DeepSeek), Wire::OpenAi);
        assert!(Engine::Claude.subscription_capable() && !Engine::DeepSeek.subscription_capable());
        assert!(Engine::Gemini.subscription_capable() && Engine::Gemini.vision());
        assert_eq!(Engine::wire(Engine::Gemini), Wire::Gemini);
    }

    #[test]
    fn effort_maps_to_each_provider() {
        assert_eq!(Effort::Off.openai(), None);
        assert_eq!((Effort::Low.claude_budget(), Effort::Max.claude_budget()), (Some(1024), Some(16384)));
        assert_eq!((Effort::Off.claude_effort(), Effort::Max.claude_effort()), ("low", "max"));
        assert_eq!((Effort::Off.gemini_budget(), Effort::Max.gemini_budget()), (0, -1));
    }

    #[test]
    fn efforts_keep_one_choice_per_engine() {
        let mut e = Efforts::default();
        e.set(Engine::Claude, Effort::High);
        assert_eq!((e.get(Engine::Claude), e.get(Engine::DeepSeek)), (Effort::High, Effort::Max));
    }
}
