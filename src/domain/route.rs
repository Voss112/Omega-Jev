use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{skill::Skill, tool::ToolKind};

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
}
