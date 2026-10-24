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
    /// Routine work a Flash model does as well: tests (unit, integration, end-to-end), code comments, summaries.
    #[serde(default)]
    pub routine: f32,
    /// Visual/UI design work (layouts, components, animations, shaders, styling).
    #[serde(default)]
    pub design: f32,
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

    pub fn needs_design(&self) -> bool {
        self.design >= 0.6
    }

    /// Lazy tool loading: only what this turn needs.
    pub fn tools(&self) -> BTreeSet<ToolKind> {
        let mut set: BTreeSet<ToolKind> = self.skill.tools().iter().copied().collect();
        let extras: [(bool, &[ToolKind]); 7] = [
            (self.needs_web(), ToolKind::WEB),
            (self.needs_web(), &[ToolKind::Kits]),
            (self.needs_code(), ToolKind::READ),
            (self.wide_impact(), &[ToolKind::BlastRadius]),
            (self.risky() && self.needs_code(), &[ToolKind::SecurityScan]),
            (self.needs_apps(), &[ToolKind::CliAnything]),
            (self.needs_design(), &[ToolKind::Kits]),
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

    /// Tests, comments or summaries with nothing security-sensitive or API-breaking in them.
    pub fn routine_task(&self) -> bool {
        self.routine >= 0.7 && !self.risky() && !self.wide_impact() && !self.needs_apps()
    }

    /// How hard the turn is, 0 (display a text) → 1 (deep, risky or wide change). Drives the Claude tier.
    pub fn difficulty(&self) -> f32 {
        let mut d = self.effort.unwrap_or(0.5);
        if self.simple >= 0.8 {
            d = d.min(0.2);
        }
        if matches!(self.skill, Skill::Security | Skill::Plan | Skill::Debug | Skill::Refactor) {
            d = d.max(0.5);
        }
        if self.risky() {
            d += 0.25;
        }
        if self.wide_impact() {
            d += 0.25;
        }
        d.clamp(0.0, 1.0)
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
            routine: 0.0,
            design: 0.0,
        }
    }

    #[test]
    fn routine_tasks_exclude_risky_and_wide_changes() {
        let tests = Route { routine: 0.9, code: 0.8, ..chat() };
        assert!(tests.routine_task());
        assert!(!Route { risk: 0.8, ..tests.clone() }.routine_task());
        assert!(!Route { blast: 0.8, ..tests.clone() }.routine_task());
        assert!(!Route { routine: 0.5, ..tests }.routine_task());
    }

    #[test]
    fn difficulty_rises_with_risk_and_falls_for_simple_turns() {
        let text = Route { effort: Some(0.1), simple: 0.9, ..chat() };
        assert!(text.difficulty() < 0.25);
        let audit = Route { skill: Skill::Security, effort: Some(0.2), ..chat() };
        assert!(audit.difficulty() >= 0.75, "{}", audit.difficulty());
        assert!((chat().difficulty() - 0.5).abs() < 1e-6, "sans estimation : milieu");
        let design = Route { design: 0.9, ..chat() };
        assert!(design.tools().contains(&ToolKind::Kits));
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
        assert_eq!((r.effort, r.simple, r.routine, r.design), (None, 0.0, 0.0, 0.0));
    }
}
