use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::traits::SkillsProvider;
use crate::types::agent::{SkillMeta, SkillMetaNode};

/// Information about a skill without loading its full content.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub path: PathBuf,
    pub source: String,
}

/// File-based skills loader that implements progressive disclosure.
///
/// Skills are loaded from the workspace/skills directory. Each skill is a
/// subdirectory containing a SKILL.md file with optional frontmatter metadata.
#[derive(Debug, Clone)]
pub struct SkillsLoader {
    workspace_skills: PathBuf,
}

impl SkillsLoader {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace_skills: workspace.join("skills"),
        }
    }

    pub async fn list_skills(&self, filter_unavailable: bool) -> Vec<SkillInfo> {
        let mut skills = Vec::new();

        if tokio::fs::try_exists(&self.workspace_skills)
            .await
            .unwrap_or(false)
        {
            let mut entries = match tokio::fs::read_dir(&self.workspace_skills).await {
                Ok(entries) => entries,
                Err(_) => return Vec::new(),
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let dir = entry.path();
                let is_dir = entry
                    .file_type()
                    .await
                    .map(|ft| ft.is_dir())
                    .unwrap_or(false);
                if !is_dir {
                    continue;
                }

                let file = dir.join("SKILL.md");
                if tokio::fs::try_exists(&file).await.unwrap_or(false) {
                    let name = dir
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default()
                        .to_string();
                    skills.push(SkillInfo {
                        name,
                        path: file,
                        source: "workspace".to_string(),
                    });
                }
            }
        }

        if filter_unavailable {
            let mut filtered = Vec::new();
            for skill in skills {
                let meta = self.get_skill_meta(&skill.name).await;
                if self.check_requirements(&meta) {
                    filtered.push(skill);
                }
            }
            filtered
        } else {
            skills
        }
    }

    pub async fn load_skill(&self, name: &str) -> Option<String> {
        let workspace = self.workspace_skills.join(name).join("SKILL.md");
        if tokio::fs::try_exists(&workspace).await.unwrap_or(false) {
            return tokio::fs::read_to_string(workspace).await.ok();
        }

        None
    }

    pub async fn get_always_skills(&self) -> Vec<String> {
        let mut always_skills = Vec::new();

        for skill in self.list_skills(true).await {
            let Some(frontmatter) = self.get_skill_metadata(&skill.name).await else {
                continue;
            };
            let skill_meta = self.parse_skill_meta(
                frontmatter
                    .get("metadata")
                    .map(|s| s.as_str())
                    .unwrap_or(""),
            );
            let always = if skill_meta.always {
                true
            } else {
                frontmatter
                    .get("always")
                    .map(|v| v == "true")
                    .unwrap_or(false)
            };
            if always {
                always_skills.push(skill.name);
            }
        }

        always_skills
    }

    pub async fn load_skills_for_context(&self, skill_names: &[String]) -> String {
        let mut parts = Vec::new();
        for name in skill_names {
            if let Some(content) = self.load_skill(name).await {
                parts.push(format!(
                    "### Skill: {}\n\n{}",
                    name,
                    strip_frontmatter(&content)
                ));
            }
        }
        parts.join("\n\n---\n\n")
    }

    pub async fn build_skills_summary(&self) -> String {
        let all = self.list_skills(false).await;
        if all.is_empty() {
            return String::new();
        }

        let mut lines = vec!["<skills>".to_string()];
        for skill in all {
            let desc = self
                .get_skill_metadata(&skill.name)
                .await
                .and_then(|m| m.get("description").cloned())
                .unwrap_or_else(|| skill.name.clone());
            let meta = self.get_skill_meta(&skill.name).await;
            let available = self.check_requirements(&meta);

            lines.push(format!(
                "  <skill available=\"{}\">",
                if available { "true" } else { "false" }
            ));
            lines.push(format!("    <name>{}</name>", xml_escape(&skill.name)));
            lines.push(format!(
                "    <description>{}</description>",
                xml_escape(&desc)
            ));
            lines.push(format!(
                "    <location>{}</location>",
                xml_escape(&skill.path.display().to_string())
            ));

            if !available {
                let missing = self.missing_requirements(&meta);
                if !missing.is_empty() {
                    lines.push(format!(
                        "    <requires>{}</requires>",
                        xml_escape(&missing.join(", "))
                    ));
                }
            }

            lines.push("  </skill>".to_string());
        }
        lines.push("</skills>".to_string());
        lines.join("\n")
    }

    async fn get_skill_meta(&self, name: &str) -> SkillMeta {
        let frontmatter = self.get_skill_metadata(name).await;
        let raw = frontmatter
            .and_then(|m| m.get("metadata").cloned())
            .unwrap_or_default();
        self.parse_skill_meta(&raw)
    }

    fn parse_skill_meta(&self, raw: &str) -> SkillMeta {
        let node = serde_json::from_str::<SkillMetaNode>(raw).unwrap_or_default();
        node.normalize()
    }

    fn check_requirements(&self, skill_meta: &SkillMeta) -> bool {
        let bins_ok = skill_meta
            .requires
            .bins
            .iter()
            .all(|bin| which::which(bin).is_ok());

        let env_ok = skill_meta
            .requires
            .env
            .iter()
            .all(|key| std::env::var(key).ok().is_some());

        bins_ok && env_ok
    }

    fn missing_requirements(&self, skill_meta: &SkillMeta) -> Vec<String> {
        let mut missing = Vec::new();

        for bin in &skill_meta.requires.bins {
            if which::which(bin).is_err() {
                missing.push(format!("CLI: {}", bin));
            }
        }

        for key in &skill_meta.requires.env {
            if std::env::var(key).ok().is_none() {
                missing.push(format!("ENV: {}", key));
            }
        }

        missing
    }

    async fn get_skill_metadata(&self, name: &str) -> Option<HashMap<String, String>> {
        let content = self.load_skill(name).await?;
        parse_frontmatter(&content)
    }
}

#[async_trait]
impl SkillsProvider for SkillsLoader {
    async fn list_skills(&self, filter_unavailable: bool) -> Vec<SkillInfo> {
        self.list_skills(filter_unavailable).await
    }

    async fn load_skill(&self, name: &str) -> Option<String> {
        self.load_skill(name).await
    }

    async fn get_always_skills(&self) -> Vec<String> {
        self.get_always_skills().await
    }

    async fn load_skills_for_context(&self, skill_names: &[String]) -> String {
        self.load_skills_for_context(skill_names).await
    }

    async fn build_skills_summary(&self) -> String {
        self.build_skills_summary().await
    }
}

fn parse_frontmatter(content: &str) -> Option<HashMap<String, String>> {
    if !content.starts_with("---") {
        return None;
    }
    let mut lines = content.lines();
    if lines.next()? != "---" {
        return None;
    }

    let mut meta = HashMap::new();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            meta.insert(
                k.trim().to_string(),
                v.trim().trim_matches('"').trim_matches('\'').to_string(),
            );
        }
    }
    Some(meta)
}

fn strip_frontmatter(content: &str) -> String {
    if !content.starts_with("---") {
        return content.to_string();
    }
    let mut it = content.splitn(3, "---\n");
    let _ = it.next();
    let _ = it.next();
    it.next().unwrap_or(content).trim().to_string()
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::types::agent::SkillRequirements;

    fn temp_workspace(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nanobot-rs-skills-{}-{}",
            case,
            uuid::Uuid::new_v4()
        ))
    }

    fn create_skill(workspace: &Path, name: &str, content: &str) {
        let skill_dir = workspace.join("skills").join(name);
        fs::create_dir_all(&skill_dir).expect("create skill dir");
        fs::write(skill_dir.join("SKILL.md"), content).expect("write skill");
    }

    #[test]
    fn parse_frontmatter_extracts_metadata() {
        let content = r#"---
description: Test skill
always: true
metadata: {"requires":{"bins":["git"]}}
---

# Skill content
"#;
        let meta = parse_frontmatter(content).expect("parse");
        assert_eq!(meta.get("description"), Some(&"Test skill".to_string()));
        assert_eq!(meta.get("always"), Some(&"true".to_string()));
    }

    #[test]
    fn parse_frontmatter_returns_none_without_delimiter() {
        let content = "# Just a heading\n\nNo frontmatter here.";
        assert!(parse_frontmatter(content).is_none());
    }

    #[test]
    fn strip_frontmatter_removes_yaml_block() {
        let content = r#"---
key: value
---

Content here"#;
        let stripped = strip_frontmatter(content);
        assert_eq!(stripped, "Content here");
    }

    #[test]
    fn strip_frontmatter_preserves_content_without_frontmatter() {
        let content = "# Heading\n\nContent";
        let stripped = strip_frontmatter(content);
        assert_eq!(stripped, content);
    }

    #[test]
    fn xml_escape_handles_special_chars() {
        assert_eq!(xml_escape("a&b"), "a&amp;b");
        assert_eq!(xml_escape("<tag>"), "&lt;tag&gt;");
        assert_eq!(xml_escape("a<b&c>d"), "a&lt;b&amp;c&gt;d");
    }

    #[tokio::test]
    async fn list_skills_finds_workspace_skills() {
        let workspace = temp_workspace("list");
        create_skill(&workspace, "test-skill", "# Test");

        let loader = SkillsLoader::new(&workspace);
        let skills = loader.list_skills(false).await;

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "test-skill");
        assert_eq!(skills[0].source, "workspace");

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn list_skills_only_returns_workspace_sources() {
        let workspace = temp_workspace("workspace-only");
        create_skill(&workspace, "common-skill", "# Workspace version");

        let loader = SkillsLoader::new(&workspace);
        let skills = loader.list_skills(false).await;

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "common-skill");
        assert_eq!(skills[0].source, "workspace");

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn load_skill_returns_content() {
        let workspace = temp_workspace("load");
        create_skill(&workspace, "my-skill", "# My Skill\n\nContent here");

        let loader = SkillsLoader::new(&workspace);
        let content = loader.load_skill("my-skill").await.expect("load");

        assert!(content.contains("My Skill"));
        assert!(content.contains("Content here"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn load_skill_returns_none_for_missing() {
        let workspace = temp_workspace("missing");
        fs::create_dir_all(&workspace).expect("create workspace");

        let loader = SkillsLoader::new(&workspace);
        assert!(loader.load_skill("nonexistent").await.is_none());

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn get_always_skills_filters_by_metadata() {
        let workspace = temp_workspace("always");
        create_skill(
            &workspace,
            "always-on",
            r#"---
always: true
---
# Always loaded"#,
        );
        create_skill(&workspace, "manual", "# Manual skill");

        let loader = SkillsLoader::new(&workspace);
        let always = loader.get_always_skills().await;

        assert!(always.contains(&"always-on".to_string()));
        assert!(!always.contains(&"manual".to_string()));

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn load_skills_for_context_combines_multiple() {
        let workspace = temp_workspace("context");
        create_skill(&workspace, "skill1", "# Skill 1");
        create_skill(&workspace, "skill2", "# Skill 2");

        let loader = SkillsLoader::new(&workspace);
        let context = loader
            .load_skills_for_context(&["skill1".to_string(), "skill2".to_string()])
            .await;

        assert!(context.contains("Skill: skill1"));
        assert!(context.contains("Skill: skill2"));
        assert!(context.contains("---"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn build_skills_summary_generates_xml() {
        let workspace = temp_workspace("summary");
        create_skill(
            &workspace,
            "test-skill",
            r#"---
description: A test skill
---
# Content"#,
        );

        let loader = SkillsLoader::new(&workspace);
        let summary = loader.build_skills_summary().await;

        assert!(summary.contains("<skills>"));
        assert!(summary.contains("<name>test-skill</name>"));
        assert!(summary.contains("<description>A test skill</description>"));
        assert!(summary.contains("</skills>"));

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn skill_meta_normalize_prefers_nanobot_node() {
        let node = SkillMetaNode {
            always: Some(false),
            requires: SkillRequirements::default(),
            nanobot: Some(Box::new(SkillMetaNode {
                always: Some(true),
                requires: SkillRequirements::default(),
                nanobot: None,
                openclaw: None,
            })),
            openclaw: None,
        };

        let meta = node.normalize();
        assert!(meta.always);
    }

    #[test]
    fn check_requirements_validates_bins_and_env() {
        let workspace = temp_workspace("requirements");
        let loader = SkillsLoader::new(&workspace);

        // Test with a binary that should exist
        let meta_with_sh = SkillMeta {
            always: false,
            requires: SkillRequirements {
                bins: vec!["sh".to_string()],
                env: vec![],
            },
        };
        assert!(loader.check_requirements(&meta_with_sh));

        // Test with a binary that shouldn't exist
        let meta_with_fake = SkillMeta {
            always: false,
            requires: SkillRequirements {
                bins: vec!["nonexistent-binary-xyz".to_string()],
                env: vec![],
            },
        };
        assert!(!loader.check_requirements(&meta_with_fake));

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn missing_requirements_lists_unavailable() {
        let workspace = temp_workspace("missing-req");
        let loader = SkillsLoader::new(&workspace);

        let meta = SkillMeta {
            always: false,
            requires: SkillRequirements {
                bins: vec!["fake-cli-tool".to_string()],
                env: vec!["FAKE_ENV_VAR".to_string()],
            },
        };

        let missing = loader.missing_requirements(&meta);
        assert!(missing.iter().any(|m| m.contains("fake-cli-tool")));
        assert!(missing.iter().any(|m| m.contains("FAKE_ENV_VAR")));

        let _ = fs::remove_dir_all(workspace);
    }
}
