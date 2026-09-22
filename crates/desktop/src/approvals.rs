//! Persisted "always allow" approval rules for the desktop.
//!
//! When the user chooses "Always allow" for a tool, it is remembered per project
//! so the approval prompt does not repeat for that tool in that repository.

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
    /// Loads `<config>/oxide/desktop/approvals.json`.
    pub fn load() -> Self {
        let path = default_path().unwrap_or_else(|_| PathBuf::from("desktop-approvals.json"));
        Self::load_from(path)
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

/// `<config>/oxide/desktop/approvals.json`, beside the project registry.
pub fn default_path() -> Result<PathBuf> {
    let projects = crate::manager::default_store_path()?;
    let dir = projects
        .parent()
        .context("resolving the desktop config directory")?;
    Ok(dir.join("approvals.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_is_scoped_per_project_and_persisted() {
        let dir = std::env::temp_dir().join(format!("oxide_approvals_{}", std::process::id()));
        let path = dir.join("approvals.json");
        let _ = std::fs::remove_dir_all(&dir);

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

        // Survives a reload.
        let reloaded = ApprovalStore::load_from(path);
        assert!(reloaded.is_allowed(&project, "bash"));

        let mut store = reloaded;
        store.clear(&project).unwrap();
        assert!(!store.is_allowed(&project, "bash"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
