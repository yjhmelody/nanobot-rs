//! Skill metadata types used for loading and resolving context files.
//!
//! This module defines the data model for skill frontmatter — the
//! YAML/TOML/JSON metadata block at the top of a skill definition.
//! Skills are agent-role definitions (similar to system prompts) that
//! can be injected into the agent's context on demand or always.
//!
//! # Key design decisions
//!
//! - [`SkillMetaNode`] supports nested overrides (`nanobot`/`openclaw`
//!   fields) so that a shared skill definition can carry platform-specific
//!   variations. [`SkillMetaNode::normalize`] resolves these to a flat
//!   [`SkillMeta`].
//! - Default values are controlled via `#[serde(default)]` so that
//!   partially-specified frontmatter parses without error.

use serde::Deserialize;

/// Runtime requirements parsed from skill frontmatter.
///
/// These describe what external resources a skill needs to run. The system
/// checks requirements before loading the skill and reports missing
/// prerequisites to the user.
///
/// # Fields
///
/// * `bins` — Executable binaries that must be available on `PATH`.
/// * `env` — Environment variables that must be set.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SkillRequirements {
    /// Executable binaries that must be available on `PATH`.
    pub bins: Vec<String>,
    /// Environment variables that must be set.
    pub env: Vec<String>,
}

/// Raw skill metadata node that may contain nested overrides.
///
/// Each skill can carry platform-specific overrides under the `nanobot`
/// or `openclaw` keys. When present, these nested nodes shadow the parent
/// metadata for that platform.
///
/// # Resolution order
///
/// 1. If `nanobot` is present, use its contents (recursively resolved).
/// 2. Otherwise, if `openclaw` is present, use its contents.
/// 3. Otherwise, use the top-level fields directly.
///
/// # Serde behaviour
///
/// `#[serde(default)]` allows any or all fields to be absent in the source
/// data. The `bool` fields are `Option`-wrapped so that the absence of the
/// field can be distinguished from `false` during override resolution.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SkillMetaNode {
    /// If `true`, the skill is always injected regardless of context.
    pub always: Option<bool>,
    /// Runtime requirements for this skill.
    pub requires: SkillRequirements,
    /// Optional nanobot-specific metadata overrides.
    ///
    /// When present, this entire node replaces the parent during
    /// resolution on the nanobot platform.
    pub nanobot: Option<Box<SkillMetaNode>>,
    /// Optional openclaw-specific metadata overrides.
    ///
    /// When present, this entire node replaces the parent during
    /// resolution on the openclaw platform.
    pub openclaw: Option<Box<SkillMetaNode>>,
}

/// Normalized skill metadata after resolving platform overrides.
///
/// This is the flattened, platform-specific output produced by
/// [`SkillMetaNode::normalize`]. Unlike [`SkillMetaNode`], the boolean
/// fields are non-optional because overrides must have been resolved.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SkillMeta {
    /// Whether the skill should always be injected.
    ///
    /// Skills with `always: true` are unconditionally included in the
    /// agent's context, regardless of tool routing or relevance scoring.
    pub always: bool,
    /// Runtime requirements after override resolution.
    ///
    /// These are the final requirements for the current platform after
    /// merging any platform-specific overrides.
    pub requires: SkillRequirements,
}

impl SkillMetaNode {
    /// Resolves platform-specific overrides (`nanobot`/`openclaw`) and
    /// returns the final `SkillMeta`.
    ///
    /// # Resolution logic
    ///
    /// 1. If a `nanobot` override exists, it is returned (recursively
    ///    normalised).
    /// 2. Otherwise, if an `openclaw` override exists, it is returned.
    /// 3. Otherwise, the top-level fields are used directly, with `always`
    ///    defaulting to `false` if absent.
    pub fn normalize(self) -> SkillMeta {
        // Platform-specific overrides take priority over the top-level node.
        // This allows a shared skill definition to carry different settings
        // for nanobot vs openclaw without duplicating the entire metadata.
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
