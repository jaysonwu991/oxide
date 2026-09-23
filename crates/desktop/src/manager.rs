//! Multi-project state for the desktop app.
//!
//! The desktop shows every project the user has touched — both folders they
//! explicitly added and projects discovered from the shared session store — and
//! lists each project's sessions. It reuses the CLI's on-disk configuration
//! (`config.json`, `auth.json`, `settings.json`) and session tree, so the two
//! front-ends see the same data.

use anyhow::{Context, Result};
use oxide_core::config::Config;
use oxide_core::session::{SessionLog, SessionSummary};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A folder the user added to the desktop, persisted across launches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    /// Stable identity: the canonical project path.
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    #[serde(default)]
    pub added_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_opened_at: Option<u64>,
}

/// A project as rendered in the sidebar: registered folders plus folders
/// discovered from sessions, annotated with activity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectView {
    pub path: String,
    pub name: String,
    /// Whether the folder was explicitly added (vs. only seen in a session).
    pub registered: bool,
    pub exists: bool,
    pub session_count: usize,
    pub last_session_at: u64,
    pub last_opened_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectRegistry {
    pub projects: Vec<Project>,
}

impl ProjectRegistry {
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("parsing desktop projects at {}", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }
}

/// Owns the persisted project registry and derives the sidebar overview from
/// the shared session store.
#[derive(Debug, Clone)]
pub struct DesktopManager {
    registry: ProjectRegistry,
    store: PathBuf,
}

impl DesktopManager {
    /// Loads the registry from `<config>/Oxide/desktop/projects.json`, sharing
    /// the Oxide config directory with the CLI.
    pub fn load() -> Result<Self> {
        Self::load_from(default_store_path()?)
    }

    pub fn load_from(store: PathBuf) -> Result<Self> {
        let registry = ProjectRegistry::load_from(&store)?;
        Ok(Self { registry, store })
    }

    pub fn store_path(&self) -> &Path {
        &self.store
    }

    pub fn registered(&self) -> &[Project] {
        &self.registry.projects
    }

    fn persist(&self) -> Result<()> {
        self.registry.save_to(&self.store)
    }

    /// Adds a folder, canonicalizing it so the same project added twice (or via
    /// a symlink) resolves to one entry.
    pub fn add_project(&mut self, path: &Path) -> Result<Project> {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("resolving {}", path.display()))?;
        if !canonical.is_dir() {
            anyhow::bail!("{} is not a directory", canonical.display());
        }
        if let Some(existing) = self
            .registry
            .projects
            .iter_mut()
            .find(|project| project.path == canonical)
        {
            existing.last_opened_at = Some(now_secs());
            let found = existing.clone();
            self.persist()?;
            return Ok(found);
        }
        let project = Project {
            id: canonical.to_string_lossy().to_string(),
            name: display_name(&canonical),
            path: canonical,
            added_at: now_secs(),
            last_opened_at: Some(now_secs()),
        };
        self.registry.projects.push(project.clone());
        self.persist()?;
        Ok(project)
    }

    /// Removes a folder from the sidebar. Sessions on disk are left untouched.
    pub fn remove_project(&mut self, id: &str) -> Result<bool> {
        let before = self.registry.projects.len();
        self.registry
            .projects
            .retain(|project| project.id != id && project.path.to_string_lossy() != id);
        let removed = self.registry.projects.len() != before;
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    /// Records that a project was just opened.
    pub fn touch(&mut self, id: &str) -> Result<()> {
        if let Some(project) = self
            .registry
            .projects
            .iter_mut()
            .find(|project| project.id == id)
        {
            project.last_opened_at = Some(now_secs());
            self.persist()?;
        }
        Ok(())
    }

    /// Sessions for one project, newest first.
    pub fn sessions_for(&self, project: &Path) -> Result<Vec<SessionSummary>> {
        SessionLog::list(project)
    }

    /// Every session across every project, newest first.
    pub fn all_sessions(&self) -> Result<Vec<SessionSummary>> {
        SessionLog::list_all()
    }

    /// The CLI configuration for a project: the same global `config.json` and
    /// `auth.json`, with that project's ecosystem loaded and project trust
    /// resolved exactly as the CLI does before running.
    pub fn config_for(&self, project: &Path) -> Result<Config> {
        load_project_config(project)
    }

    /// Reads the shared session store and merges it with the registry.
    pub fn overview(&self) -> Result<Vec<ProjectView>> {
        Ok(self.overview_with(&self.all_sessions()?))
    }

    /// Pure merge of registered projects and session-derived projects. Kept
    /// separate from disk I/O so it can be tested directly.
    pub fn overview_with(&self, sessions: &[SessionSummary]) -> Vec<ProjectView> {
        let mut views: Vec<ProjectView> = Vec::new();

        for project in &self.registry.projects {
            let path = project.path.to_string_lossy().to_string();
            let (count, last) = session_stats(sessions, &project.path);
            views.push(ProjectView {
                exists: project.path.is_dir(),
                path,
                name: project.name.clone(),
                registered: true,
                session_count: count,
                last_session_at: last,
                last_opened_at: project.last_opened_at,
            });
        }

        for summary in sessions {
            let path = PathBuf::from(&summary.cwd);
            if views.iter().any(|view| view.path == summary.cwd) {
                continue;
            }
            // One row per distinct session cwd not already registered.
            let (count, last) = session_stats(sessions, &path);
            views.push(ProjectView {
                exists: path.is_dir(),
                name: display_name(&path),
                path: summary.cwd.clone(),
                registered: false,
                session_count: count,
                last_session_at: last,
                last_opened_at: None,
            });
        }

        // Registered projects first (by last opened), then discovered ones by
        // most recent activity.
        views.sort_by(|a, b| {
            b.registered
                .cmp(&a.registered)
                .then(
                    b.last_opened_at
                        .unwrap_or(0)
                        .cmp(&a.last_opened_at.unwrap_or(0)),
                )
                .then(b.last_session_at.cmp(&a.last_session_at))
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        views
    }
}

/// Loads the CLI configuration for a project, applying the same project-trust
/// resolution as the CLI before the agent runs.
pub fn load_project_config(project: &Path) -> Result<Config> {
    load_project_config_with(project, None)
}

/// Like [`load_project_config`], with an optional per-run `reasoning` override
/// (the same value `oxide --reasoning` accepts).
pub fn load_project_config_with(project: &Path, reasoning: Option<String>) -> Result<Config> {
    let mut config = Config::load(project, None, None, None, reasoning)?;
    let store = oxide_core::trust::TrustStore::load().unwrap_or_default();
    config.trusted =
        oxide_core::trust::resolve(&store, project, None, config.default_project_trust)
            .is_trusted();
    if !config.trusted {
        config.reload_ecosystem(project);
    }
    Ok(config)
}

fn session_stats(sessions: &[SessionSummary], path: &Path) -> (usize, u64) {
    let path = path.to_string_lossy();
    sessions
        .iter()
        .filter(|summary| summary.cwd == path)
        .fold((0, 0), |(count, last), summary| {
            (count + 1, last.max(summary.modified_at))
        })
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// `<config>/Oxide/desktop/projects.json`, alongside the CLI's `config.json`.
pub fn default_store_path() -> Result<PathBuf> {
    let config = Config::config_path();
    let dir = config
        .parent()
        .context("resolving the Oxide config directory")?;
    Ok(dir.join("desktop/projects.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_store() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oxide_desktop_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("desktop/projects.json")
    }

    fn summary(cwd: &str, modified: u64) -> SessionSummary {
        SessionSummary {
            id: format!("s{modified}"),
            name: None,
            cwd: cwd.to_string(),
            created_at: modified,
            modified_at: modified,
            message_count: 1,
            preview: "hi".to_string(),
            path: PathBuf::from(format!("/tmp/{modified}.jsonl")),
        }
    }

    #[test]
    fn add_is_stable_and_deduped() {
        let store = temp_store();
        let mut manager = DesktopManager::load_from(store.clone()).unwrap();
        let dir = std::env::temp_dir().join(format!("oxide_proj_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let first = manager.add_project(&dir).unwrap();
        let second = manager.add_project(&dir).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(manager.registered().len(), 1);

        // Persisted across reloads.
        let reloaded = DesktopManager::load_from(store).unwrap();
        assert_eq!(reloaded.registered().len(), 1);

        std::fs::remove_dir_all(&dir).ok();
        let _ = std::fs::remove_dir_all(manager.store_path().parent().unwrap());
    }

    #[test]
    fn remove_drops_the_entry() {
        let store = temp_store();
        let mut manager = DesktopManager::load_from(store).unwrap();
        let dir = std::env::temp_dir().join(format!("oxide_proj_rm_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = manager.add_project(&dir).unwrap();
        assert!(manager.remove_project(&project.id).unwrap());
        assert!(manager.registered().is_empty());
        assert!(!manager.remove_project("nothing").unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overview_merges_registered_and_discovered_projects() {
        let store = temp_store();
        let mut manager = DesktopManager::load_from(store).unwrap();
        let dir = std::env::temp_dir().join(format!("oxide_proj_ov_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = manager.add_project(&dir).unwrap();

        let registered = project.path.to_string_lossy().to_string();
        let sessions = vec![
            summary(&registered, 10),
            summary(&registered, 30),
            summary("/tmp/from-terminal", 20),
        ];

        let views = manager.overview_with(&sessions);
        assert_eq!(views.len(), 2);
        let reg = views.iter().find(|view| view.registered).unwrap();
        assert_eq!(reg.session_count, 2);
        assert_eq!(reg.last_session_at, 30);
        let disc = views.iter().find(|view| !view.registered).unwrap();
        assert_eq!(disc.name, "from-terminal");
        assert_eq!(disc.session_count, 1);

        // Registered projects sort ahead of discovered ones.
        assert!(views[0].registered);
        std::fs::remove_dir_all(&dir).ok();
    }
}
