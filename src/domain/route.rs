use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{engine::Effort, skill::Skill, tool::ToolKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Jev,
    Local,
}

/// One typed routing verdict per user turn (System One output).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Route {
    pub skill: Skill,
    pub source: Source,
    pub confidence: f32,
    pub web: f32,
    pub code: f32,
    pub risk: f32,
    pub blast: f32,
    /// Needs to drive a real application (CLI-Anything).
    #[serde(default)]
    pub apps: f32,
    /// Superpowers playbook id injected this turn.
    #[serde(default)]
    pub playbook: Option<String>,
    /// Reasoning depth the request needs, 0 (trivial) → 1 (deep); `None` = not estimated.
    #[serde(default)]
    pub effort: Option<f32>,
    /// Plain chat or factual question that needs no tools, files or deep reasoning.
    #[serde(default)]
    pub simple: f32,
}

impl Route {
    pub fn needs_web(&self) -> bool {
        self.web >= 0.6 || self.skill == Skill::Research
    }

    pub fn needs_code(&self) -> bool {
        self.code >= 0.5
    }

    pub fn risky(&self) -> bool {
        self.risk >= 0.5 || self.skill == Skill::Security
    }

    pub fn wide_impact(&self) -> bool {
        self.blast >= 0.6
    }

    pub fn needs_apps(&self) -> bool {
        self.apps >= 0.5
    }

    /// Lazy tool loading: only what this turn needs.
    pub fn tools(&self) -> BTreeSet<ToolKind> {
        let mut set: BTreeSet<ToolKind> = self.skill.tools().iter().copied().collect();
        let extras: [(bool, &[ToolKind]); 5] = [
            (self.needs_web(), ToolKind::WEB),
            (self.needs_code(), ToolKind::READ),
            (self.wide_impact(), &[ToolKind::BlastRadius]),
            (self.risky() && self.needs_code(), &[ToolKind::SecurityScan]),
            (self.needs_apps(), &[ToolKind::CliAnything]),
        ];
        for (on, kinds) in extras {
            if on {
                set.extend(kinds);
            }
        }
        set
    }

    /// Effort for this turn: Jev's estimate, never above what the user chose (`None` = the user's choice).
    pub fn effort_cap(&self, chosen: Effort) -> Effort {
        let Some(score) = self.effort else { return chosen };
        let estimated = match score {
            s if s < 0.25 => Effort::Low,
            s if s < 0.5 => Effort::Medium,
            s if s < 0.75 => Effort::High,
            _ => Effort::Max,
        };
        estimated.min(chosen)
    }

    /// A simple, tool-free, methodology-free turn: a cheaper engine answers it as well.
    pub fn delegable(&self) -> bool {
        self.simple >= 0.8 && self.tools().is_empty() && self.playbook.is_none() && !self.risky() && !self.needs_apps()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat() -> Route {
        Route {
            skill: Skill::Chat,
            source: Source::Jev,
            confidence: 0.9,
            web: 0.0,
            code: 0.0,
            risk: 0.0,
            blast: 0.0,
            apps: 0.0,
            playbook: None,
            effort: None,
            simple: 0.0,
        }
    }

    #[test]
    fn effort_cap_never_raises_the_users_choice() {
        let r = |e: f32| Route { effort: Some(e), ..chat() };
        assert_eq!(r(0.1).effort_cap(Effort::High), Effort::Low);
        assert_eq!(r(0.9).effort_cap(Effort::Medium), Effort::Medium);
        assert_eq!(r(0.6).effort_cap(Effort::Max), Effort::High);
        assert_eq!(r(0.1).effort_cap(Effort::Off), Effort::Off);
        assert_eq!(chat().effort_cap(Effort::High), Effort::High);
    }

    #[test]
    fn delegable_requires_a_simple_toolless_turn() {
        let simple = Route { simple: 0.9, ..chat() };
        assert!(simple.delegable());
        assert!(!Route { code: 0.6, ..simple.clone() }.delegable());
        assert!(!Route { playbook: Some("brainstorming".into()), ..simple.clone() }.delegable());
        assert!(!Route { simple: 0.7, ..simple }.delegable());
    }

    #[test]
    fn old_routes_without_effort_still_load() {
        let old = r#"{"skill":"chat","source":"Jev","confidence":0.8,"web":0,"code":0,"risk":0,"blast":0}"#;
        let r: Route = serde_json::from_str(old).unwrap();
        assert_eq!((r.effort, r.simple), (None, 0.0));
    }
}
