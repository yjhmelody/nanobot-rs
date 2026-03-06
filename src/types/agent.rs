use std::collections::BTreeMap;

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

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum PreviewValue {
    String(String),
    Bool(bool),
    Integer(i64),
    Float(f64),
    Array(Vec<PreviewValue>),
    Object(BTreeMap<String, PreviewValue>),
}

impl PreviewValue {
    pub(crate) fn short(&self) -> String {
        match self {
            Self::String(s) => s.clone(),
            Self::Bool(v) => v.to_string(),
            Self::Integer(v) => v.to_string(),
            Self::Float(v) => v.to_string(),
            Self::Array(values) => values
                .first()
                .map(|v| v.short())
                .unwrap_or_else(|| "[]".to_string()),
            Self::Object(map) => map
                .iter()
                .next()
                .map(|(_, v)| v.short())
                .unwrap_or_else(|| "{}".to_string()),
        }
    }
}
