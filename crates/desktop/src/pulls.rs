//! The pull requests each stored thread reported, and the state last read for
//! them.
//!
//! Reading a thread's log for its links is what lets the sidebar carry a row's
//! state, so the answer is kept beside the project registry and refreshed only
//! when the thread's log moved. The state itself comes from the forge's CLI,
//! which is slow enough that it is read off the command's own thread and
//! announced when it lands.

use oxide_core::pulls;
use oxide_core::session::SessionSummary;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// How long a state read from a forge stands before it is read again.
pub const STATE_TTL: u64 = 300;

/// A thread's pull request as a row draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullView {
    pub url: String,
    pub label: String,
    /// `open`, `merged` or `closed`; empty while the forge has not been read.
    pub state: String,
}

/// What a thread reported, keyed by the log it was read from.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ThreadLinks {
    modified_at: u64,
    urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedState {
    state: String,
    checked_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Cache {
    sessions: HashMap<String, ThreadLinks>,
    states: HashMap<String, CachedState>,
}

#[derive(Debug)]
pub struct PullStore {
    path: PathBuf,
    cache: Mutex<Cache>,
    /// Held by the one call reading the forge, so a listing that arrives while it
    /// runs does not start a second round of CLI processes.
    refreshing: AtomicBool,
}

impl PullStore {
    pub fn open() -> Self {
        Self::open_at(oxide_core::session_state::path().with_file_name("pulls.json"))
    }

    pub fn open_at(path: PathBuf) -> Self {
        let cache = Cache::load(&path);
        Self {
            path,
            cache: Mutex::new(cache),
            refreshing: AtomicBool::new(false),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The requests each of these threads reported, with the state last read for
    /// them. A thread whose log moved is read again; the rest come from the
    /// cache, so a listing costs one file per changed thread.
    pub fn annotate(&self, sessions: &[SessionSummary]) -> HashMap<String, Vec<PullView>> {
        let mut cache = self.lock();
        let mut wrote = false;
        for summary in sessions {
            let known = cache
                .sessions
                .get(&summary.id)
                .is_some_and(|entry| entry.modified_at == summary.modified_at);
            if known {
                continue;
            }
            let urls = links_in(&summary.path);
            cache.sessions.insert(
                summary.id.clone(),
                ThreadLinks {
                    modified_at: summary.modified_at,
                    urls,
                },
            );
            wrote = true;
        }
        if wrote {
            cache.save(&self.path);
        }
        sessions
            .iter()
            .map(|summary| (summary.id.clone(), cache.views_for(summary)))
            .collect()
    }

    /// The URLs whose state is not known yet or was read longer ago than
    /// [`STATE_TTL`], once each.
    pub fn stale(&self, urls: &[String]) -> Vec<String> {
        let cache = self.lock();
        let now = now_secs();
        let mut stale: Vec<String> = Vec::new();
        for url in urls {
            if stale.contains(url) {
                continue;
            }
            let fresh = cache
                .states
                .get(url)
                .is_some_and(|state| now.saturating_sub(state.checked_at) < STATE_TTL);
            if !fresh {
                stale.push(url.clone());
            }
        }
        stale
    }

    /// Whether this call is the one that reads the forge.
    pub fn begin_refresh(&self) -> bool {
        !self.refreshing.swap(true, Ordering::AcqRel)
    }

    pub fn end_refresh(&self) {
        self.refreshing.store(false, Ordering::Release);
    }

    /// Reads each URL's state through its forge CLI, returning whether any of
    /// them answered with a state this store did not already hold.
    pub fn refresh(&self, urls: &[String]) -> bool {
        let mut changed = false;
        for url in urls {
            let Some(pull) = pulls::link(url) else {
                continue;
            };
            let Some(state) = pulls::read_state(&pull) else {
                continue;
            };
            self.lock().states.insert(
                url.clone(),
                CachedState {
                    state: state.as_str().to_string(),
                    checked_at: now_secs(),
                },
            );
            changed = true;
        }
        if changed {
            self.lock().save(&self.path);
        }
        changed
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|error| error.into_inner())
    }
}

impl Cache {
    fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) {
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    fn views_for(&self, summary: &SessionSummary) -> Vec<PullView> {
        self.sessions
            .get(&summary.id)
            .map(|entry| entry.urls.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|url| {
                let pull = pulls::link(url)?;
                let state = self
                    .states
                    .get(url)
                    .map(|state| state.state.clone())
                    .unwrap_or_default();
                Some(PullView {
                    url: pull.url,
                    label: pull.label,
                    state,
                })
            })
            .collect()
    }
}

/// The requests a thread's log names. A log that cannot be read is a thread with
/// no links, which is also what an empty one holds.
fn links_in(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    pulls::links(&text)
        .into_iter()
        .map(|pull| pull.url)
        .collect()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "oxide-pulls-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn summary(id: &str, path: &Path, modified_at: u64) -> SessionSummary {
        SessionSummary {
            id: id.to_string(),
            name: None,
            cwd: "/tmp/project".to_string(),
            created_at: modified_at,
            modified_at,
            message_count: 1,
            preview: String::new(),
            path: path.to_path_buf(),
        }
    }

    #[test]
    fn a_thread_is_read_for_its_links_once_until_its_log_moves() {
        let dir = temp_dir();
        let log = dir.join("thread.jsonl");
        std::fs::write(
            &log,
            "{\"text\":\"opened https://github.com/owner/repo/pull/7\"}",
        )
        .unwrap();
        let store = PullStore::open_at(dir.join("pulls.json"));
        let thread = summary("t1", &log, 10);
        let views = store.annotate(std::slice::from_ref(&thread));
        assert_eq!(
            views["t1"],
            vec![PullView {
                url: "https://github.com/owner/repo/pull/7".to_string(),
                label: "#7".to_string(),
                state: String::new(),
            }]
        );
        // The link is cached against the log's own timestamp, so a log that is
        // deleted after being read still lists the row it was read for.
        std::fs::remove_file(&log).unwrap();
        let again = PullStore::open_at(dir.join("pulls.json"));
        assert_eq!(again.annotate(std::slice::from_ref(&thread))["t1"].len(), 1);
        // A log that moved is read again, and this one no longer names anything.
        assert!(again.annotate(&[summary("t1", &log, 11)])["t1"].is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_state_last_read_long_ago_is_stale_again() {
        let dir = temp_dir();
        let cache = json!({
            "sessions": {},
            "states": {
                "https://github.com/owner/repo/pull/7": { "state": "merged", "checked_at": now_secs() },
                "https://github.com/owner/repo/pull/8": { "state": "open", "checked_at": now_secs() - STATE_TTL - 1 },
            },
        });
        std::fs::write(dir.join("pulls.json"), cache.to_string()).unwrap();
        let store = PullStore::open_at(dir.join("pulls.json"));
        let urls = vec![
            "https://github.com/owner/repo/pull/7".to_string(),
            "https://github.com/owner/repo/pull/8".to_string(),
            "https://github.com/owner/repo/pull/9".to_string(),
            "https://github.com/owner/repo/pull/9".to_string(),
        ];
        assert_eq!(
            store.stale(&urls),
            vec![
                "https://github.com/owner/repo/pull/8".to_string(),
                "https://github.com/owner/repo/pull/9".to_string(),
            ]
        );
        assert!(store.begin_refresh() && !store.begin_refresh());
        store.end_refresh();
        assert!(store.begin_refresh());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_state_of_a_link_a_row_names_is_the_state_that_was_read() {
        let dir = temp_dir();
        let log = dir.join("thread.jsonl");
        std::fs::write(
            &log,
            "https://github.com/owner/repo/pull/7\nhttps://gitlab.com/group/app/-/merge_requests/2\n",
        )
        .unwrap();
        let cache = json!({
            "sessions": {},
            "states": {
                "https://github.com/owner/repo/pull/7": { "state": "merged", "checked_at": now_secs() },
                "https://gitlab.com/group/app/-/merge_requests/2": { "state": "closed", "checked_at": now_secs() },
            },
        });
        std::fs::write(dir.join("pulls.json"), cache.to_string()).unwrap();
        let store = PullStore::open_at(dir.join("pulls.json"));
        let views = store.annotate(&[summary("t1", &log, 3)]);
        assert_eq!(
            views["t1"]
                .iter()
                .map(|pull| (pull.label.as_str(), pull.state.as_str()))
                .collect::<Vec<_>>(),
            vec![("#7", "merged"), ("!2", "closed")]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
