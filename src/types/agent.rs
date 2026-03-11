use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SkillRequirements {
    pub(crate) bins: Vec<String>,
    pub(crate) env: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SkillMetaNode {
    pub(crate) always: Option<bool>,
    pub(crate) requires: SkillRequirements,
    pub(crate) nanobot: Option<Box<SkillMetaNode>>,
    pub(crate) openclaw: Option<Box<SkillMetaNode>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct SkillMeta {
    pub(crate) always: bool,
    pub(crate) requires: SkillRequirements,
}

impl SkillMetaNode {
    pub(crate) fn normalize(self) -> SkillMeta {
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
