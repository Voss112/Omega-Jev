//! Superpowers playbooks (obra/superpowers, MIT) embedded verbatim; Jev loads at most one per turn.

pub struct Playbook {
    pub id: &'static str,
    source: &'static str,
}

macro_rules! playbooks {
    ($($id:literal),* $(,)?) => {
        [$(Playbook { id: $id, source: include_str!(concat!("../../assets/skills/superpowers/", $id, ".md")) }),*]
    };
}

pub const ALL: [Playbook; 15] = playbooks!(
    "brainstorming",
    "diagnosing-superpowers",
    "dispatching-parallel-agents",
    "executing-plans",
    "finishing-a-development-branch",
    "receiving-code-review",
    "requesting-code-review",
    "subagent-driven-development",
    "systematic-debugging",
    "test-driven-development",
    "using-git-worktrees",
    "using-superpowers",
    "verification-before-completion",
    "writing-plans",
    "writing-skills",
);

impl Playbook {
    pub fn find(id: &str) -> Option<&'static Self> {
        ALL.iter().find(|p| p.id == id)
    }

    /// `description:` from the YAML frontmatter (used as Jev choice criteria).
    pub fn description(&self) -> &'static str {
        self.source.lines().find_map(|l| l.strip_prefix("description:")).map_or("", |d| d.trim().trim_matches('"'))
    }

    /// Markdown body without the frontmatter.
    pub fn body(&self) -> &'static str {
        self.source.strip_prefix("---").and_then(|rest| rest.split_once("\n---")).map_or(self.source, |(_, body)| body).trim()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_playbook_has_description_and_body() {
        for p in &super::ALL {
            assert!(!p.description().is_empty(), "{} has no description", p.id);
            assert!(!p.body().starts_with("---") && p.body().len() > 500, "{} body", p.id);
        }
    }
}
