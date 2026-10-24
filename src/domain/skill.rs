use serde::{Deserialize, Serialize};

use super::tool::ToolKind::{self, *};

/// Micro-skills (Superpowers / Trail of Bits / Strix condensed). Jev picks exactly one per turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Skill {
    #[default]
    Chat,
    Code,
    Tdd,
    Debug,
    Review,
    Security,
    Refactor,
    Research,
    Plan,
    Ops,
}

struct Spec {
    id: &'static str,
    label: &'static str,
    criteria: &'static str,
    prompt: &'static str,
    tools: &'static [ToolKind],
    /// Superpowers playbook used when Jev is unavailable.
    playbook: Option<&'static str>,
}

const CODE_TOOLS: &[ToolKind] = &[ListDir, ReadFile, Outline, FindSymbol, Grep, EditFile, ReplaceSymbol, WriteFile, Shell];

const SPECS: [Spec; 10] = [
    Spec {
        id: "chat",
        label: "Discussion",
        criteria: "General question, explanation, advice or conversation; no file changes",
        prompt: "Direct answer. Example only if it clarifies.",
        tools: &[],
        playbook: None,
    },
    Spec {
        id: "code",
        label: "Code",
        criteria: "Implement a feature, write or modify code, create a program or file",
        prompt: "1) locate via outline/find_symbol 2) smallest correct change 3) build+test via shell 4) one-line result.",
        tools: CODE_TOOLS,
        playbook: Some("verification-before-completion"),
    },
    Spec {
        id: "tdd",
        label: "TDD",
        criteria: "Write tests, test-driven development, increase coverage",
        prompt: "Red→green→refactor. Write failing test, run it and see it fail, minimal code to pass, rerun, refactor. Never skip the failing run.",
        tools: CODE_TOOLS,
        playbook: Some("test-driven-development"),
    },
    Spec {
        id: "debug",
        label: "Debug",
        criteria: "Fix a bug, error, crash, panic, failing test or unexpected behavior",
        prompt: "Systematic: reproduce → read error verbatim → root-cause hypothesis → prove with evidence (logs/tests) → fix root cause, not symptom → regression test. No guess-fixes.",
        tools: &[ListDir, ReadFile, Outline, FindSymbol, Grep, BlastRadius, EditFile, ReplaceSymbol, Shell],
        playbook: Some("systematic-debugging"),
    },
    Spec {
        id: "review",
        label: "Revue",
        criteria: "Review code or a diff for quality, correctness and maintainability",
        prompt: "Check correctness, edge cases, errors, security, perf, readability. Output: path:line — problem — fix, severity-ordered. No praise.",
        tools: &[ListDir, ReadFile, Outline, FindSymbol, Grep, BlastRadius, SecurityScan, Shell],
        playbook: Some("requesting-code-review"),
    },
    Spec {
        id: "security",
        label: "Audit sécurité",
        criteria: "Security audit, vulnerabilities, pentest, secrets, hardening, threat model",
        prompt: "Map attack surface: inputs, authn/z, crypto, FFI/unsafe, shell, deserialization, SSRF, path traversal, injection, secrets, deps. Run security_scan, confirm each finding by reading code. Each: severity, CWE, exploit scenario, fix. Unconfirmed = SUSPECT. Zero false positives.",
        tools: &[ListDir, ReadFile, Outline, FindSymbol, Grep, BlastRadius, SecurityScan, Shell, WebSearch, AuditKit],
        playbook: Some("security-audit"),
    },
    Spec {
        id: "refactor",
        label: "Refactor",
        criteria: "Restructure, rename, clean up or simplify code without changing behavior",
        prompt: "Behavior-preserving. blast_radius before touching public signatures; update all callers; tests before and after.",
        tools: &[ListDir, ReadFile, Outline, FindSymbol, Grep, BlastRadius, EditFile, ReplaceSymbol, WriteFile, Shell],
        playbook: Some("verification-before-completion"),
    },
    Spec {
        id: "research",
        label: "Recherche",
        criteria: "Find information, documentation, versions, news, comparisons on the web",
        prompt: "docs_search (local cache) first, then web_search, then web_fetch with focus. Cross-check ≥2 sources, prefer official docs, cite URLs, state versions/dates.",
        tools: &[DocsSearch, WebSearch, WebFetch],
        playbook: None,
    },
    Spec {
        id: "plan",
        label: "Architecture",
        criteria: "Design, architecture, plan, brainstorm, choose technologies",
        prompt: "Clarify goal+constraints. 2-3 options with trade-offs, recommend one. Then step plan: files, interfaces, verification. DDD/hexagonal when it fits, YAGNI.",
        tools: &[ListDir, ReadFile, Outline, Grep, WebSearch],
        playbook: Some("brainstorming"),
    },
    Spec {
        id: "ops",
        label: "Ops",
        criteria: "Run commands, build, install, git, docker, deploy, system administration",
        prompt: "Exact commands for this OS. Inspect state before changing. Prefer reversible steps. Explain destructive ones before running.",
        tools: &[ListDir, ReadFile, Grep, EditFile, WriteFile, Shell],
        playbook: None,
    },
];

impl Skill {
    pub const ALL: [Skill; 10] = [
        Self::Chat,
        Self::Code,
        Self::Tdd,
        Self::Debug,
        Self::Review,
        Self::Security,
        Self::Refactor,
        Self::Research,
        Self::Plan,
        Self::Ops,
    ];

    fn spec(self) -> &'static Spec {
        &SPECS[self as usize]
    }

    pub fn id(self) -> &'static str {
        self.spec().id
    }

    pub fn label(self) -> &'static str {
        let label = self.spec().label;
        match label {
            "Revue" => crate::ui::tr(label, "Review"),
            "Audit sécurité" => crate::ui::tr(label, "Security audit"),
            "Recherche" => crate::ui::tr(label, "Research"),
            _ => label,
        }
    }

    pub fn criteria(self) -> &'static str {
        self.spec().criteria
    }

    pub fn prompt(self) -> &'static str {
        self.spec().prompt
    }

    pub fn tools(self) -> &'static [ToolKind] {
        self.spec().tools
    }

    pub fn default_playbook(self) -> Option<&'static str> {
        self.spec().playbook
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_skill_uses_the_cloudflare_playbook() {
        assert_eq!(Skill::Security.default_playbook(), Some("security-audit"));
        assert!(Skill::Security.tools().contains(&ToolKind::SecurityScan));
        assert!(crate::domain::playbook::Playbook::find("security-audit").is_some());
    }
}
