use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub path: PathBuf,
    pub source: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct SkillRequirements {
    bins: Vec<String>,
    env: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct SkillMetaNode {
    always: Option<bool>,
    requires: SkillRequirements,
    nanobot: Option<Box<SkillMetaNode>>,
    openclaw: Option<Box<SkillMetaNode>>,
}

#[derive(Debug, Clone, Default)]
struct SkillMeta {
    always: bool,
    requires: SkillRequirements,
}

impl SkillMetaNode {
    fn normalize(self) -> SkillMeta {
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

pub struct SkillsLoader {
    workspace_skills: PathBuf,
    builtin_skills: PathBuf,
}

impl SkillsLoader {
    pub fn new(workspace: &Path) -> Self {
        let builtin_skills = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("nanobot")
            .join("skills");
        Self {
            workspace_skills: workspace.join("skills"),
            builtin_skills,
        }
    }

    pub fn list_skills(&self, filter_unavailable: bool) -> Vec<SkillInfo> {
        let mut skills = Vec::new();

        if self.workspace_skills.exists() {
            for entry in WalkDir::new(&self.workspace_skills)
                .min_depth(1)
                .max_depth(1)
                .into_iter()
                .flatten()
            {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let file = dir.join("SKILL.md");
                if file.exists() {
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

        if self.builtin_skills.exists() {
            for entry in WalkDir::new(&self.builtin_skills)
                .min_depth(1)
                .max_depth(1)
                .into_iter()
                .flatten()
            {
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                let file = dir.join("SKILL.md");
                if !file.exists() {
                    continue;
                }
                let name = dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();

                if skills.iter().any(|s| s.name == name) {
                    continue;
                }
                skills.push(SkillInfo {
                    name,
                    path: file,
                    source: "builtin".to_string(),
                });
            }
        }

        if filter_unavailable {
            skills
                .into_iter()
                .filter(|s| self.check_requirements(&self.get_skill_meta(&s.name)))
                .collect()
        } else {
            skills
        }
    }

    pub fn load_skill(&self, name: &str) -> Option<String> {
        let workspace = self.workspace_skills.join(name).join("SKILL.md");
        if workspace.exists() {
            return fs::read_to_string(workspace).ok();
        }

        let builtin = self.builtin_skills.join(name).join("SKILL.md");
        if builtin.exists() {
            return fs::read_to_string(builtin).ok();
        }

        None
    }

    pub fn get_always_skills(&self) -> Vec<String> {
        self.list_skills(true)
            .into_iter()
            .filter_map(|s| {
                let frontmatter = self.get_skill_metadata(&s.name)?;
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
                if always { Some(s.name) } else { None }
            })
            .collect()
    }

    pub fn load_skills_for_context(&self, skill_names: &[String]) -> String {
        let mut parts = Vec::new();
        for name in skill_names {
            if let Some(content) = self.load_skill(name) {
                parts.push(format!(
                    "### Skill: {}\n\n{}",
                    name,
                    strip_frontmatter(&content)
                ));
            }
        }
        parts.join("\n\n---\n\n")
    }

    pub fn build_skills_summary(&self) -> String {
        let all = self.list_skills(false);
        if all.is_empty() {
            return String::new();
        }

        let mut lines = vec!["<skills>".to_string()];
        for skill in all {
            let desc = self
                .get_skill_metadata(&skill.name)
                .and_then(|m| m.get("description").cloned())
                .unwrap_or_else(|| skill.name.clone());
            let meta = self.get_skill_meta(&skill.name);
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

    fn get_skill_meta(&self, name: &str) -> SkillMeta {
        let frontmatter = self.get_skill_metadata(name);
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

    fn get_skill_metadata(&self, name: &str) -> Option<HashMap<String, String>> {
        let content = self.load_skill(name)?;
        parse_frontmatter(&content)
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
