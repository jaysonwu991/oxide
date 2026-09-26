//! Persisted "always allow" tool approvals, shared by every front-end.
//!
//! When the user answers an approval prompt with "Always allow", the tool is
//! remembered per project so the prompt does not repeat for that tool in that
//! repository. The file lives beside `config.json` in the Oxide config
//! directory, so the terminal, the desktop app and the VS Code extension all
//! see the same rules.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Rules {
    /// Project path → allowed tool names.
    #[serde(default)]
    allow: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone)]
pub struct ApprovalStore {
    path: PathBuf,
    rules: Rules,
}

impl ApprovalStore {
    /// Loads `<config>/Oxide/approvals.json`, migrating the desktop's older
    /// private file (`<config>/Oxide/desktop/approvals.json`) on first use.
    pub fn load() -> Self {
        let mut store = Self::load_from(default_path());
        if store.rules.allow.is_empty() {
            let legacy = Self::load_from(legacy_path());
            if !legacy.rules.allow.is_empty() {
                store.rules = legacy.rules;
                let _ = store.save();
            }
        }
        store
    }

    pub fn load_from(path: PathBuf) -> Self {
        let rules = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { path, rules }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_allowed(&self, project: &Path, tool: &str) -> bool {
        self.rules
            .allow
            .get(&key(project))
            .is_some_and(|tools| tools.contains(tool))
    }

    pub fn allow(&mut self, project: &Path, tool: &str) -> Result<()> {
        self.rules
            .allow
            .entry(key(project))
            .or_default()
            .insert(tool.to_string());
        self.save()
    }

    pub fn list(&self, project: &Path) -> Vec<String> {
        self.rules
            .allow
            .get(&key(project))
            .map(|tools| tools.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn clear(&mut self, project: &Path) -> Result<()> {
        self.rules.allow.remove(&key(project));
        self.save()
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&self.rules)?;
        std::fs::write(&self.path, text).with_context(|| format!("writing {}", self.path.display()))
    }
}

fn key(project: &Path) -> String {
    project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf())
        .to_string_lossy()
        .to_string()
}

/// `<config>/Oxide/approvals.json`, beside `config.json`.
pub fn default_path() -> PathBuf {
    crate::config::config_dir_or_default().join("approvals.json")
}

/// The desktop's older per-app file, read once so rules saved before the store
/// was shared are not lost.
fn legacy_path() -> PathBuf {
    crate::config::config_dir_or_default().join("desktop/approvals.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn allow_is_scoped_per_project_and_persisted() {
        let dir = temp_dir("approvals");
        let path = dir.join("approvals.json");

        let project = dir.join("proj");
        let other = dir.join("other");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&other).unwrap();

        let mut store = ApprovalStore::load_from(path.clone());
        assert!(!store.is_allowed(&project, "bash"));
        store.allow(&project, "bash").unwrap();
        assert!(store.is_allowed(&project, "bash"));
        assert!(!store.is_allowed(&other, "bash"));
        assert_eq!(store.list(&project), vec!["bash".to_string()]);

        let reloaded = ApprovalStore::load_from(path);
        assert!(reloaded.is_allowed(&project, "bash"));

        let mut store = reloaded;
        store.clear(&project).unwrap();
        assert!(!store.is_allowed(&project, "bash"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_rule_survives_a_second_tool_in_the_same_project() {
        let dir = temp_dir("approvals_multi");
        std::fs::create_dir_all(&dir).unwrap();
        let project = dir.join("proj");
        std::fs::create_dir_all(&project).unwrap();

        let path = dir.join("approvals.json");
        let mut store = ApprovalStore::load_from(path.clone());
        store.allow(&project, "edit").unwrap();
        store.allow(&project, "bash").unwrap();
        assert_eq!(
            store.list(&project),
            vec!["bash".to_string(), "edit".to_string()]
        );

        let reloaded = ApprovalStore::load_from(path);
        assert!(reloaded.is_allowed(&project, "edit"));
        assert!(reloaded.is_allowed(&project, "bash"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_reads_as_empty() {
        let dir = temp_dir("approvals_missing");
        let store = ApprovalStore::load_from(dir.join("approvals.json"));
        assert!(!store.is_allowed(&dir, "bash"));
        assert!(store.list(&dir).is_empty());
    }
}
