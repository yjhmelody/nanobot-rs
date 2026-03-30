use serde::Deserialize;

/// Runtime requirements parsed from skill frontmatter.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SkillRequirements {
    /// Executable binaries that must be available on `PATH`.
    pub bins: Vec<String>,
    /// Environment variables that must be set.
    pub env: Vec<String>,
}

/// Raw skill metadata node that may contain nested overrides.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SkillMetaNode {
    /// If `true`, the skill is always injected regardless of context.
    pub always: Option<bool>,
    /// Runtime requirements for this skill.
    pub requires: SkillRequirements,
    /// Optional nanobot-specific metadata overrides.
    pub nanobot: Option<Box<SkillMetaNode>>,
    /// Optional openclaw-specific metadata overrides.
    pub openclaw: Option<Box<SkillMetaNode>>,
}

/// Normalized skill metadata after resolving overrides.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SkillMeta {
    /// Whether the skill should always be injected.
    pub always: bool,
    /// Runtime requirements after override resolution.
    pub requires: SkillRequirements,
}

impl SkillMetaNode {
    /// Resolves nanobot/openclaw overrides and returns the final `SkillMeta`.
    pub fn normalize(self) -> SkillMeta {
        if let Some(node) = self.nanobot {
            return node.normalize();
        }
        if let Some(node) = self.openclaw {
            return node.normalize();
        }
        SkillMeta {
            always: self.always.unwrap_or(false),
            requires: self.requires,
        }
    }
}
