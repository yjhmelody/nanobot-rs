//! Workspace template definitions.
//!
//! These constants define the default files that are synchronised into the
//! workspace directory by `sync_workspace_templates`. Each `TemplateFile`
//! pairs a relative path with content embedded at compile time via
//! `include_str!`.

/// A workspace template file with a relative path and embedded content.
pub struct TemplateFile {
    /// Relative path within the workspace (e.g., `"AGENTS.md"`).
    pub rel_path: &'static str,
    /// File content, embedded at compile time via `include_str!`.
    pub content: &'static str,
}

/// Root-level workspace templates (placed directly in the workspace).
pub const ROOT_TEMPLATES: &[TemplateFile] = &[
    TemplateFile {
        rel_path: "AGENTS.md",
        content: include_str!("../../templates/AGENTS.md"),
    },
    TemplateFile {
        rel_path: "SOUL.md",
        content: include_str!("../../templates/SOUL.md"),
    },
    TemplateFile {
        rel_path: "USER.md",
        content: include_str!("../../templates/USER.md"),
    },
    TemplateFile {
        rel_path: "TOOLS.md",
        content: include_str!("../../templates/TOOLS.md"),
    },
    TemplateFile {
        rel_path: "HEARTBEAT.md",
        content: include_str!("../../templates/HEARTBEAT.md"),
    },
];

/// Memory template placed in `memory/MEMORY.md` within the workspace.
pub const MEMORY_TEMPLATE: TemplateFile = TemplateFile {
    rel_path: "memory/MEMORY.md",
    content: include_str!("../../templates/memory/MEMORY.md"),
};

/// Relative path to the HISTORY.md file (starts empty, populated at runtime).
pub const HISTORY_TEMPLATE_PATH: &str = "memory/HISTORY.md";
