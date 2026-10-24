use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{
    auth::AuthMode,
    engine::{Efforts, Engine},
};

/// Output verbosity (caveman family). Shorter = fewer output tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    Normal,
    Concise,
    Caveman,
    /// Default: maximum output-token economy.
    #[default]
    Ultra,
}

impl Style {
    pub const ALL: [Style; 4] = [Self::Normal, Self::Concise, Self::Caveman, Self::Ultra];

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Concise => "Concis",
            Self::Caveman => "Caveman",
            Self::Ultra => "Ultra-caveman",
        }
    }

    pub fn prompt(self) -> &'static str {
        match self {
            Self::Normal => "Style: clear, structured, complete.",
            Self::Concise => "Style: concise. Bullets over paragraphs. Only changed code.",
            Self::Caveman => {
                "Style: terse like smart caveman. Drop articles, filler, pleasantries, hedging; fragments OK. \
                 Keep every technical fact; code, numbers, errors exact. Only changed code."
            }
            Self::Ultra => {
                "Style: ultra-terse telegraphic. Abbreviate common terms (DB, fn, cfg, req, impl). \
                 Arrows for causality (X → Y). One word when enough. Code/errors verbatim."
            }
        }
    }
}

/// Flat user-facing configuration (one toggle per option is intended).
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Active System Two engine (switchable from the composer).
    pub engine: Engine,
    pub deepseek_key: String,
    pub deepseek_url: String,
    pub deepseek_model: String,
    pub jev_key: String,
    pub jev_url: String,
    pub jev_model: String,
    pub claude_key: String,
    pub claude_url: String,
    pub claude_model: String,
    pub claude_auth: AuthMode,
    /// Effort chosen per engine (never by the router).
    pub effort: Efforts,
    pub style: Style,
    pub max_steps: u32,
    pub auto_approve: bool,
    /// Bypass permissions: every approval is auto-granted (hard-blocked commands stay blocked).
    pub bypass: bool,
    /// Ask before every file change (diff shown).
    pub confirm_edits: bool,
    /// Run the project's checker (cargo check, tsc…) after edits and feed errors back.
    pub auto_diagnostics: bool,
    /// Summarize older history once a request exceeds this many prompt tokens.
    pub compact_at: u64,
    pub proxy: String,
    pub require_vpn: bool,
    pub ip_lookup: bool,
    pub workspace: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            engine: Engine::default(),
            deepseek_key: String::new(),
            deepseek_url: "https://api.deepseek.com".into(),
            deepseek_model: "deepseek-flash".into(),
            jev_key: String::new(),
            jev_url: "https://api.typesafe.ai/v1/systemone".into(),
            jev_model: "jev-latest".into(),
            claude_key: String::new(),
            claude_url: "https://api.anthropic.com".into(),
            claude_model: "claude-sonnet-4-5".into(),
            claude_auth: AuthMode::Subscription,
            effort: Efforts::default(),
            style: Style::default(),
            max_steps: 64,
            auto_approve: true,
            bypass: false,
            confirm_edits: false,
            auto_diagnostics: true,
            compact_at: 100_000,
            proxy: String::new(),
            require_vpn: false,
            // Off by default: no third party learns the exit IP unless the user opts in.
            ip_lookup: false,
            workspace: None,
        }
    }
}

impl Settings {
    /// (url, key, model) configured for `engine`.
    pub fn endpoint(&self, engine: Engine) -> (&str, &str, &str) {
        let model = self.model(engine);
        match engine {
            Engine::DeepSeek => (&self.deepseek_url, &self.deepseek_key, model),
            Engine::Claude => (&self.claude_url, &self.claude_key, model),
        }
    }

    /// Model id sent to `engine` (the picker's source of truth).
    pub fn model(&self, engine: Engine) -> &str {
        match engine {
            Engine::DeepSeek => &self.deepseek_model,
            Engine::Claude => &self.claude_model,
        }
    }

    /// Keeps one model per engine, so switching provider never loses the choice.
    pub fn set_model(&mut self, engine: Engine, id: &str) {
        let slot = match engine {
            Engine::DeepSeek => &mut self.deepseek_model,
            Engine::Claude => &mut self.claude_model,
        };
        slot.clear();
        slot.push_str(id.trim());
    }

    /// Auth mode of `engine` (API keys are the only option for DeepSeek).
    pub fn auth_mode(&self, engine: Engine) -> AuthMode {
        match engine {
            Engine::DeepSeek => AuthMode::ApiKey,
            Engine::Claude => self.claude_auth,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_settings_file_still_loads() {
        let old = r#"
engine = "deep_seek"
kimi_url = "https://api.moonshot.cn/v1"
kimi_key = ""
compact_at = 120000
max_steps = 24
"#;
        let s: Settings = toml::from_str(old).expect("rétrocompatible");
        assert_eq!(s.engine, Engine::DeepSeek);
        assert_eq!(s.claude_auth, AuthMode::Subscription, "les abonnements sont le défaut");
        assert_eq!(s.effort.get(Engine::DeepSeek), Engine::DeepSeek.default_effort());
        assert_eq!(s.claude_url, "https://api.anthropic.com");
        let hand_edited: Settings = toml::from_str("engine = \"deepseek\"\n").expect("alias toléré");
        assert_eq!(hand_edited.engine, Engine::DeepSeek);
    }

    #[test]
    fn retired_engines_fall_back_to_their_closest_successor() {
        let kimi: Settings =
            toml::from_str("engine = \"kimi\"\nkimi_key = \"x\"\n[effort]\nkimi = \"high\"\n").expect("kimi retiré");
        assert_eq!(kimi.engine, Engine::DeepSeek);
        let gemini: Settings = toml::from_str("engine = \"gemini\"\ngemini_auth = \"api_key\"\n").expect("gemini retiré");
        assert_eq!(gemini.engine, Engine::Claude);
    }

    #[test]
    fn endpoint_and_auth_mode_follow_the_engine() {
        let s = Settings::default();
        assert_eq!(s.endpoint(Engine::Claude).0, "https://api.anthropic.com");
        assert_eq!(s.auth_mode(Engine::DeepSeek), AuthMode::ApiKey);
        assert_eq!(s.auth_mode(Engine::Claude), AuthMode::Subscription);
    }

    #[test]
    fn each_engine_keeps_its_own_model() {
        let mut s = Settings::default();
        s.set_model(Engine::Claude, " claude-opus-5-5 ");
        assert_eq!(s.claude_model, "claude-opus-5-5");
        assert_eq!(s.endpoint(Engine::Claude).2, "claude-opus-5-5");
        assert_eq!(s.model(Engine::DeepSeek), "deepseek-flash", "les autres moteurs ne bougent pas");
        let back: Settings = toml::from_str(&toml::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.model(Engine::Claude), "claude-opus-5-5");
    }
}
