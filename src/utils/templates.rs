pub struct TemplateFile {
    pub rel_path: &'static str,
    pub content: &'static str,
}

pub const ROOT_TEMPLATES: &[TemplateFile] = &[
    TemplateFile {
        rel_path: "AGENTS.md",
        content: include_str!("../../../nanobot/templates/AGENTS.md"),
    },
    TemplateFile {
        rel_path: "SOUL.md",
        content: include_str!("../../../nanobot/templates/SOUL.md"),
    },
    TemplateFile {
        rel_path: "USER.md",
        content: include_str!("../../../nanobot/templates/USER.md"),
    },
    TemplateFile {
        rel_path: "TOOLS.md",
        content: include_str!("../../../nanobot/templates/TOOLS.md"),
    },
    TemplateFile {
        rel_path: "HEARTBEAT.md",
        content: include_str!("../../../nanobot/templates/HEARTBEAT.md"),
    },
];

pub const MEMORY_TEMPLATE: TemplateFile = TemplateFile {
    rel_path: "memory/MEMORY.md",
    content: include_str!("../../../nanobot/templates/memory/MEMORY.md"),
};

pub const HISTORY_TEMPLATE_PATH: &str = "memory/HISTORY.md";
