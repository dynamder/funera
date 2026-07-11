#![cfg(feature = "skill")]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,
    pub disable_model_invocation: bool,
    pub source_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: Option<String>,
    #[serde(rename = "disable-model-invocation")]
    disable_model_invocation: Option<bool>,
}

impl Skill {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            content: content.into(),
            disable_model_invocation: false,
            source_path: None,
        }
    }

    pub fn new_with_config(
        name: impl Into<String>,
        description: impl Into<String>,
        content: impl Into<String>,
        disable_model_invocation: bool,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            content: content.into(),
            disable_model_invocation,
            source_path: None,
        }
    }

    pub fn from_str(s: &str) -> Result<Self, SkillParseError> {
        let s = s.trim();
        if !s.starts_with("---") {
            return Err(SkillParseError::MissingFrontmatter);
        }

        let end = s[3..]
            .find("\n---")
            .map(|pos| pos + 3)
            .ok_or(SkillParseError::UnclosedFrontmatter)?;

        let yaml_str = &s[3..end].trim();
        let frontmatter: SkillFrontmatter =
            serde_yaml::from_str(yaml_str).map_err(SkillParseError::YamlError)?;

        let body = s[(end + 4)..].trim();

        Ok(Self {
            name: frontmatter.name,
            description: frontmatter.description.unwrap_or_default(),
            content: body.to_string(),
            disable_model_invocation: frontmatter.disable_model_invocation.unwrap_or(false),
            source_path: None,
        })
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, SkillParseError> {
        let path = path.as_ref();
        let content = fs::read_to_string(path).map_err(SkillParseError::IoError)?;
        let mut skill = Self::from_str(&content)?;
        skill.source_path = Some(path.to_path_buf());
        Ok(skill)
    }

    pub fn from_dir(path: impl AsRef<Path>) -> Result<Vec<Self>, SkillParseError> {
        let path = path.as_ref();
        let mut skills = Vec::new();

        if !path.is_dir() {
            return Err(SkillParseError::NotADirectory(path.to_path_buf()));
        }

        for entry in fs::read_dir(path).map_err(SkillParseError::IoError)? {
            let entry = entry.map_err(SkillParseError::IoError)?;
            let file_path = entry.path();

            if file_path.extension().and_then(|e| e.to_str()) == Some("md") {
                match Self::from_file(&file_path) {
                    Ok(skill) => skills.push(skill),
                    Err(e) => {
                        eprintln!("warn: skipping skill file {:?}: {}", file_path, e);
                    }
                }
            }
        }

        Ok(skills)
    }

    pub fn from_default_path() -> Vec<Self> {
        let candidates = [
            std::env::var("SKILLS_HOME").ok().map(PathBuf::from),
            dirs_home_dir().map(|mut p| {
                p.push(".agents");
                p.push("skills");
                p
            }),
        ];

        let mut all = Vec::new();
        for candidate in candidates.iter().flatten() {
            if candidate.is_dir() {
                if let Ok(skills) = Self::from_dir(candidate) {
                    all.extend(skills);
                }
            }
        }
        all
    }
}

fn dirs_home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillParseError {
    #[error("missing YAML frontmatter (---)")]
    MissingFrontmatter,
    #[error("unclosed YAML frontmatter")]
    UnclosedFrontmatter,
    #[error("YAML parse error: {0}")]
    YamlError(#[from] serde_yaml::Error),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("not a directory: {0}")]
    NotADirectory(PathBuf),
}

#[derive(Debug, Clone)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
    active: HashSet<String>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
            active: HashSet::new(),
        }
    }

    pub fn add(&mut self, skill: Skill) {
        self.skills.insert(skill.name.clone(), skill);
    }

    pub fn remove(&mut self, name: &str) -> Option<Skill> {
        self.active.remove(name);
        self.skills.remove(name)
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.skills.contains_key(name)
    }

    pub fn is_active(&self, name: &str) -> bool {
        self.active.contains(name)
    }

    pub fn activate(&mut self, name: &str) -> bool {
        if self.skills.contains_key(name) {
            self.active.insert(name.to_string());
            true
        } else {
            false
        }
    }

    pub fn deactivate(&mut self, name: &str) -> bool {
        self.active.remove(name)
    }

    pub fn all_skills(&self) -> &HashMap<String, Skill> {
        &self.skills
    }

    pub fn active_skills(&self) -> impl Iterator<Item = &Skill> {
        self.active.iter().filter_map(|name| self.skills.get(name))
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    pub fn total_count(&self) -> usize {
        self.skills.len()
    }

    pub fn get_active_skills_prompt(&self) -> String {
        let mut parts = Vec::new();
        for skill in self.active_skills() {
            if !skill.disable_model_invocation {
                if !skill.content.is_empty() {
                    parts.push(skill.content.clone());
                }
            }
        }
        parts.join("\n\n")
    }

    pub fn get_active_skills_metadata(&self) -> Vec<(&str, &str)> {
        self.active_skills()
            .filter(|s| !s.disable_model_invocation)
            .map(|s| (s.name.as_str(), s.description.as_str()))
            .collect()
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skill_minimal() {
        let raw = "---\nname: test-skill\ndescription: A test\n---\n\n# Test Skill\n\nHello world";
        let skill = Skill::from_str(raw).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.description, "A test");
        assert!(!skill.disable_model_invocation);
        assert!(skill.content.contains("Test Skill"));
        assert!(skill.content.contains("Hello world"));
    }

    #[test]
    fn parse_skill_all_fields() {
        let raw = "---\nname: my-skill\ndescription: Does something\ndisable-model-invocation: true\n---\n\nDo the thing.";
        let skill = Skill::from_str(raw).unwrap();
        assert_eq!(skill.name, "my-skill");
        assert_eq!(skill.description, "Does something");
        assert!(skill.disable_model_invocation);
        assert_eq!(skill.content, "Do the thing.");
    }

    #[test]
    fn parse_skill_no_description() {
        let raw = "---\nname: bare\n---\n\ncontent here";
        let skill = Skill::from_str(raw).unwrap();
        assert_eq!(skill.name, "bare");
        assert_eq!(skill.description, "");
        assert!(!skill.disable_model_invocation);
    }

    #[test]
    fn parse_skill_missing_frontmatter() {
        let err = Skill::from_str("no frontmatter here").unwrap_err();
        assert!(matches!(err, SkillParseError::MissingFrontmatter));
    }

    #[test]
    fn parse_skill_unclosed_frontmatter() {
        let err = Skill::from_str("---\nname: bad\nstill open").unwrap_err();
        assert!(matches!(err, SkillParseError::UnclosedFrontmatter));
    }

    #[test]
    fn registry_add_and_get() {
        let mut reg = SkillRegistry::new();
        let skill = Skill::new("s1", "skill one", "do stuff");
        reg.add(skill);
        assert!(reg.contains("s1"));
        assert_eq!(reg.get("s1").unwrap().name, "s1");
        assert_eq!(reg.total_count(), 1);
    }

    #[test]
    fn registry_remove() {
        let mut reg = SkillRegistry::new();
        reg.add(Skill::new("s1", "", ""));
        assert!(reg.remove("s1").is_some());
        assert!(!reg.contains("s1"));
    }

    #[test]
    fn registry_activate_deactivate() {
        let mut reg = SkillRegistry::new();
        reg.add(Skill::new("s1", "", "content a"));
        reg.add(Skill::new("s2", "", "content b"));

        assert!(!reg.is_active("s1"));
        assert!(reg.activate("s1"));
        assert!(reg.is_active("s1"));
        assert!(!reg.is_active("s2"));

        assert!(reg.deactivate("s1"));
        assert!(!reg.is_active("s1"));
    }

    #[test]
    fn registry_activate_nonexistent() {
        let mut reg = SkillRegistry::new();
        assert!(!reg.activate("ghost"));
    }

    #[test]
    fn registry_active_skills_prompt() {
        let mut reg = SkillRegistry::new();
        reg.add(Skill::new("s1", "", "part one"));
        reg.add(Skill::new_with_config("s2", "", "part two", true));
        reg.add(Skill::new("s3", "", "part three"));

        reg.activate("s1");
        reg.activate("s2");
        reg.activate("s3");

        let prompt = reg.get_active_skills_prompt();
        assert!(prompt.contains("part one"));
        assert!(!prompt.contains("part two"));
        assert!(prompt.contains("part three"));
    }

    #[test]
    fn registry_active_skills_prompt_empty() {
        let reg = SkillRegistry::new();
        assert!(reg.get_active_skills_prompt().is_empty());
    }

    #[test]
    fn registry_active_skills_iterator() {
        let mut reg = SkillRegistry::new();
        reg.add(Skill::new("a", "", "aaa"));
        reg.add(Skill::new("b", "", "bbb"));
        reg.activate("a");

        let names: Vec<&str> = reg.active_skills().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["a"]);
    }

    #[test]
    fn registry_active_metadata() {
        let mut reg = SkillRegistry::new();
        reg.add(Skill::new("x", "desc x", "content"));
        reg.add(Skill::new("y", "desc y", "content"));
        reg.activate("x");
        reg.activate("y");

        let meta = reg.get_active_skills_metadata();
        assert_eq!(meta.len(), 2);
    }

    #[test]
    fn registry_default_empty() {
        let reg = SkillRegistry::default();
        assert_eq!(reg.total_count(), 0);
        assert_eq!(reg.active_count(), 0);
    }

    #[test]
    fn from_str_trim_whitespace() {
        let raw = "\n\n---\nname: ws\n---\n\nbody\n\n";
        let skill = Skill::from_str(raw).unwrap();
        assert_eq!(skill.name, "ws");
        assert_eq!(skill.content, "body");
    }

    #[test]
    fn from_file_reads_skill() {
        let dir = std::env::temp_dir().join(format!("skill_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("test_skill.md");
        let content = "---\nname: file-skill\ndescription: From file\n---\n\nFile content here";
        std::fs::write(&file_path, content).unwrap();

        let skill = Skill::from_file(&file_path).unwrap();
        assert_eq!(skill.name, "file-skill");
        assert_eq!(skill.description, "From file");
        assert_eq!(skill.content, "File content here");
        assert_eq!(skill.source_path, Some(file_path.clone()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_file_nonexistent_returns_error() {
        let result = Skill::from_file("/nonexistent/path/skill.md");
        assert!(result.is_err());
    }

    #[test]
    fn from_dir_reads_markdown_files() {
        let dir = std::env::temp_dir().join(format!("skills_dir_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        std::fs::write(dir.join("a.md"), "---\nname: skill-a\n---\n\nContent A").unwrap();
        std::fs::write(dir.join("b.md"), "---\nname: skill-b\n---\n\nContent B").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a skill").unwrap();

        let skills = Skill::from_dir(&dir).unwrap();
        assert_eq!(skills.len(), 2, "only .md files should be loaded");
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"skill-a"));
        assert!(names.contains(&"skill-b"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_dir_nonexistent_returns_error() {
        let result = Skill::from_dir("/nonexistent/skills/dir");
        assert!(result.is_err());
    }

    #[test]
    fn from_file_with_disable_model_invocation() {
        let dir = std::env::temp_dir().join(format!("skill_cfg_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("cfg_skill.md");
        let content = "---\nname: cfg-skill\ndescription: Configured\ndisable-model-invocation: true\n---\n\nConfig content";
        std::fs::write(&file_path, content).unwrap();

        let skill = Skill::from_file(&file_path).unwrap();
        assert!(skill.disable_model_invocation);
        assert_eq!(skill.content, "Config content");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
