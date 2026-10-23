use serde::{Deserialize, Serialize};

/// Every capability the agent can call. Order = canonical order in the request (cache-stable).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    ListDir,
    ReadFile,
    Outline,
    FindSymbol,
    Grep,
    BlastRadius,
    EditFile,
    ReplaceSymbol,
    WriteFile,
    Shell,
    SecurityScan,
    DocsSearch,
    WebSearch,
    WebFetch,
    CliAnything,
    /// Installs the six-phase security-audit skill into the workspace.
    AuditKit,
}

impl ToolKind {
    pub const ALL: [ToolKind; 16] = [
        Self::ListDir,
        Self::ReadFile,
        Self::Outline,
        Self::FindSymbol,
        Self::Grep,
        Self::BlastRadius,
        Self::EditFile,
        Self::ReplaceSymbol,
        Self::WriteFile,
        Self::Shell,
        Self::SecurityScan,
        Self::DocsSearch,
        Self::WebSearch,
        Self::WebFetch,
        Self::CliAnything,
        Self::AuditKit,
    ];
    /// Loaded whenever code is involved: navigation, impact and security analysis are all read-only.
    pub const READ: &[ToolKind] =
        &[Self::ListDir, Self::ReadFile, Self::Outline, Self::FindSymbol, Self::Grep, Self::BlastRadius, Self::SecurityScan];
    pub const WEB: &[ToolKind] = &[Self::DocsSearch, Self::WebSearch, Self::WebFetch];

    pub fn name(self) -> &'static str {
        match self {
            Self::ListDir => "list_dir",
            Self::ReadFile => "read_file",
            Self::Outline => "outline",
            Self::FindSymbol => "find_symbol",
            Self::Grep => "grep",
            Self::BlastRadius => "blast_radius",
            Self::EditFile => "edit_file",
            Self::ReplaceSymbol => "replace_symbol",
            Self::WriteFile => "write_file",
            Self::Shell => "shell",
            Self::SecurityScan => "security_scan",
            Self::DocsSearch => "docs_search",
            Self::WebSearch => "web_search",
            Self::WebFetch => "web_fetch",
            Self::CliAnything => "cli_anything",
            Self::AuditKit => "audit_kit",
        }
    }

    /// No side effect: safe to run concurrently.
    pub fn read_only(self) -> bool {
        !matches!(self, Self::EditFile | Self::ReplaceSymbol | Self::WriteFile | Self::Shell | Self::CliAnything | Self::AuditKit)
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::ListDir => "🗁",
            Self::ReadFile | Self::Outline | Self::FindSymbol => "📄",
            Self::Grep | Self::DocsSearch => "🔍",
            Self::BlastRadius => "🕸",
            Self::EditFile | Self::ReplaceSymbol | Self::WriteFile => "✏",
            Self::Shell => "▶",
            Self::SecurityScan => "🛡",
            Self::WebSearch | Self::WebFetch => "🌐",
            Self::CliAnything => "🧩",
            Self::AuditKit => "📋",
        }
    }
}
