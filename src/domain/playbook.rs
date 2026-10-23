//! Method playbooks (Superpowers MIT, Cloudflare security-audit MIT) embedded verbatim; Jev loads at most one per turn.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Superpowers,
    Security,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Self::Superpowers => "Superpowers",
            Self::Security => "Audit sécurité",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Playbook {
    pub id: &'static str,
    pub group: Group,
    source: &'static str,
}

macro_rules! playbooks {
    ($group:expr, $dir:literal, [$($id:literal),* $(,)?]) => {
        [$(Playbook { id: $id, group: $group, source: include_str!(concat!("../../assets/skills/", $dir, "/", $id, ".md")) }),*]
    };
}

const SUPERPOWERS: [Playbook; 15] = playbooks!(
    Group::Superpowers,
    "superpowers",
    [
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
    ]
);

const SECURITY: Playbook = Playbook {
    id: "security-audit",
    group: Group::Security,
    source: include_str!("../../assets/skills/security-audit/SKILL.md"),
};

pub const ALL: [Playbook; 16] = [
    SUPERPOWERS[0],
    SUPERPOWERS[1],
    SUPERPOWERS[2],
    SUPERPOWERS[3],
    SUPERPOWERS[4],
    SUPERPOWERS[5],
    SUPERPOWERS[6],
    SUPERPOWERS[7],
    SUPERPOWERS[8],
    SUPERPOWERS[9],
    SUPERPOWERS[10],
    SUPERPOWERS[11],
    SUPERPOWERS[12],
    SUPERPOWERS[13],
    SUPERPOWERS[14],
    SECURITY,
];

impl Playbook {
    pub fn find(id: &str) -> Option<&'static Self> {
        ALL.iter().find(|p| p.id == id)
    }

    /// `description:` from the YAML frontmatter, or the first prose line of a foreign skill.
    pub fn description(&self) -> &'static str {
        if let Some(d) = self.source.lines().find_map(|l| l.strip_prefix("description:")) {
            return d.trim().trim_matches('"');
        }
        self.source
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('*') && !l.starts_with("---"))
            .unwrap_or("")
    }

    /// Markdown body without the frontmatter.
    pub fn body(&self) -> &'static str {
        self.source.strip_prefix("---").and_then(|rest| rest.split_once("\n---")).map_or(self.source, |(_, body)| body).trim()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_playbook_belongs_to_a_group_and_has_a_body() {
        for p in &super::ALL {
            assert!(p.body().len() > 500, "{} body", p.id);
            assert!(!p.description().is_empty(), "{} description", p.id);
        }
    }

    #[test]
    fn the_security_audit_bundle_is_complete() {
        let p = super::Playbook::find("security-audit").expect("playbook embarque");
        assert_eq!(p.group, super::Group::Security);
        assert!(p.body().len() > 2_000, "corps trop court: {}", p.body().len());
        for needle in ["Reconnaissance", "findings.json", "coverage"] {
            assert!(p.body().contains(needle), "SKILL.md ne mentionne pas {needle}");
        }
        assert!(!p.description().is_empty());
    }
}
