//! Shared pin/archive preferences. Archiving never changes a session log.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionState {
    pub pinned_sessions: Vec<String>,
    pub archived_sessions: Vec<String>,
}

pub fn path() -> PathBuf {
    crate::config::config_dir_or_default().join("desktop/sessions.json")
}

impl SessionState {
    pub fn load_from(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("parsing session preferences"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Older desktops kept these fields in their project registry.
                match fs::read(path.with_file_name("projects.json")) {
                    Ok(bytes) => {
                        serde_json::from_slice(&bytes).context("reading legacy session preferences")
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        Ok(Self::default())
                    }
                    Err(error) => Err(error.into()),
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn pinned(&self, id: &str) -> bool {
        self.pinned_sessions.iter().any(|each| each == id)
    }

    pub fn archived(&self, id: &str) -> bool {
        self.archived_sessions.iter().any(|each| each == id)
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> Result<()> {
        anyhow::ensure!(
            !pinned || !self.archived(id),
            "Unarchive this session before pinning it."
        );
        set(&mut self.pinned_sessions, id, pinned);
        Ok(())
    }

    pub fn set_archived(&mut self, id: &str, archived: bool) {
        set(&mut self.archived_sessions, id, archived);
        if archived {
            set(&mut self.pinned_sessions, id, false);
        }
    }

    pub fn update_at(path: &Path, update: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))?;
        lock.lock()?;
        let mut state = Self::load_from(path)?;
        update(&mut state)?;
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, serde_json::to_vec_pretty(&state)?)?;
        fs::rename(&temp, path)?;
        Ok(())
    }
}

fn set(values: &mut Vec<String>, id: &str, present: bool) {
    values.retain(|each| each != id);
    if present {
        values.push(id.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archiving_preserves_the_log_and_persistent_memory() {
        let dir = std::env::temp_dir().join(format!("oxide-archive-data-{}", std::process::id()));
        let log = crate::session::SessionLog::create_in(&dir.join("logs"), &dir).unwrap();
        let original = fs::read(log.path()).unwrap();
        let memory = crate::memory::MemoryStore::open(dir.join("memory"), &dir);
        memory
            .add(
                "Keep this project note",
                crate::memory::Scope::Project,
                vec![],
            )
            .unwrap();
        let path = dir.join("sessions.json");
        SessionState::update_at(&path, |state| {
            state.set_archived(&log.summary().unwrap().id, true);
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(log.path()).unwrap(), original);
        assert!(SessionState::load_from(&path)
            .unwrap()
            .archived(&log.summary().unwrap().id));
        assert_eq!(
            crate::memory::MemoryStore::open(dir.join("memory"), &dir).len(),
            1
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migrates_desktop_preferences_and_merges_independent_clients() {
        let dir = std::env::temp_dir().join(format!("oxide-session-state-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sessions.json");
        fs::write(
            dir.join("projects.json"),
            r#"{"projects":[],"pinned_sessions":["old"],"archived_sessions":["hidden"]}"#,
        )
        .unwrap();
        SessionState::update_at(&path, |state| state.set_pinned("new", true)).unwrap();
        SessionState::update_at(&path, |state| {
            state.set_archived("old", true);
            Ok(())
        })
        .unwrap();
        let state = SessionState::load_from(&path).unwrap();
        assert!(state.archived("old") && state.archived("hidden"));
        assert!(!state.pinned("old") && state.pinned("new"));
        fs::write(dir.join("projects.json"), "{}").unwrap();
        assert!(SessionState::load_from(&path).unwrap().archived("hidden"));
        SessionState::update_at(&path, |state| {
            state.set_archived("old", false);
            state.set_pinned("old", true)
        })
        .unwrap();
        assert!(SessionState::load_from(&path).unwrap().pinned("old"));
        fs::remove_dir_all(dir).unwrap();
    }
}
