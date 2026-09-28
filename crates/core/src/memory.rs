//! Persistent memory across sessions: entries are
//! stored per project (keyed by git remote or project root) and per user, and
//! retrieved with a dependency-free lexical (tf-idf) search. The store lives
//! under `~/.config/Oxide/memory/`, never inside the repository.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const PROFILE_TAG: &str = "profile";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Project,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryScope {
    Project,
    User,
    All,
}

impl QueryScope {
    pub fn parse(value: &str) -> Self {
        match value {
            "user" => QueryScope::User,
            "all-projects" | "all" => QueryScope::All,
            _ => QueryScope::Project,
        }
    }

    pub fn entry_scope(self) -> Result<Scope> {
        match self {
            QueryScope::Project => Ok(Scope::Project),
            QueryScope::User => Ok(Scope::User),
            QueryScope::All => {
                anyhow::bail!("`add` requires scope `project` or `user`")
            }
        }
    }

    fn matches(self, scope: Scope) -> bool {
        match self {
            QueryScope::Project => scope == Scope::Project,
            QueryScope::User => scope == Scope::User,
            QueryScope::All => true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub scope: Scope,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Debug, Clone)]
pub struct MemoryStore {
    root: PathBuf,
    project_id: String,
    entries: Arc<Mutex<Vec<MemoryEntry>>>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self {
            root: PathBuf::new(),
            project_id: String::new(),
            entries: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl MemoryStore {
    pub fn load(cwd: &Path) -> Self {
        let root = crate::config::config_dir_or_default().join("memory");
        Self::open(root, cwd)
    }

    pub fn open(root: PathBuf, cwd: &Path) -> Self {
        let project_id = project_id(cwd);
        let mut entries = read_entries(&root.join("user.json"), Scope::User);
        entries.extend(read_entries(
            &project_file(&root, &project_id),
            Scope::Project,
        ));
        Self {
            root,
            project_id,
            entries: Arc::new(Mutex::new(entries)),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn add(&self, content: &str, scope: Scope, tags: Vec<String>) -> Result<MemoryEntry> {
        let now = now_secs();
        let entry = MemoryEntry {
            id: new_id(),
            content: content.trim().to_string(),
            tags,
            scope,
            created_at: now,
            updated_at: now,
        };
        let mut entries = self.entries.lock().expect("memory store poisoned");
        entries.push(entry.clone());
        self.persist(&entries, scope)?;
        Ok(entry)
    }

    pub fn search(&self, query: &str, scope: QueryScope, limit: usize) -> Vec<MemoryEntry> {
        let entries = self.entries.lock().expect("memory store poisoned");
        let docs: Vec<(&MemoryEntry, Vec<String>)> = entries
            .iter()
            .filter(|entry| scope.matches(entry.scope))
            .map(|entry| (entry, tokenize(&doc_text(entry))))
            .collect();

        let terms = tokenize(query);
        if terms.is_empty() {
            return recent(docs, limit);
        }

        let total = docs.len() as f32;
        let mut scored: Vec<(f32, &MemoryEntry)> = docs
            .iter()
            .filter_map(|(entry, tokens)| {
                let mut counts: HashMap<&str, f32> = HashMap::new();
                for token in tokens {
                    *counts.entry(token.as_str()).or_insert(0.0) += 1.0;
                }
                let mut score = 0.0f32;
                for term in &terms {
                    let Some(count) = counts.get(term.as_str()) else {
                        continue;
                    };
                    let matches = docs
                        .iter()
                        .filter(|(_, tokens)| tokens.iter().any(|token| token == term))
                        .count() as f32;
                    let idf = (1.0 + total / matches.max(1.0)).ln();
                    score += idf * (1.0 + count.ln());
                }
                if score <= 0.0 {
                    return None;
                }
                Some((score + recency_bonus(entry), *entry))
            })
            .collect();

        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, entry)| entry.clone())
            .collect()
    }

    pub fn list(&self, scope: QueryScope, limit: usize) -> Vec<MemoryEntry> {
        let entries = self.entries.lock().expect("memory store poisoned");
        let docs: Vec<(&MemoryEntry, Vec<String>)> = entries
            .iter()
            .filter(|entry| scope.matches(entry.scope))
            .map(|entry| (entry, Vec::new()))
            .collect();
        recent(docs, limit)
    }

    pub fn recent(&self, limit: usize) -> Vec<MemoryEntry> {
        self.list(QueryScope::All, limit)
    }

    pub fn profile(&self) -> Option<MemoryEntry> {
        let entries = self.entries.lock().expect("memory store poisoned");
        entries
            .iter()
            .filter(|entry| {
                entry.scope == Scope::User && entry.tags.iter().any(|t| t == PROFILE_TAG)
            })
            .max_by_key(|entry| entry.updated_at)
            .cloned()
    }

    pub fn forget(&self, id: &str) -> Result<bool> {
        let mut entries = self.entries.lock().expect("memory store poisoned");
        let Some(index) = entries.iter().position(|entry| entry.id == id) else {
            return Ok(false);
        };
        let scope = entries[index].scope;
        entries.remove(index);
        self.persist(&entries, scope)?;
        Ok(true)
    }

    fn persist(&self, entries: &[MemoryEntry], scope: Scope) -> Result<()> {
        let path = match scope {
            Scope::User => self.root.join("user.json"),
            Scope::Project => project_file(&self.root, &self.project_id),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let subset: Vec<&MemoryEntry> = entries
            .iter()
            .filter(|entry| entry.scope == scope)
            .collect();
        let raw = serde_json::to_string_pretty(&subset)?;
        std::fs::write(&path, raw)?;
        Ok(())
    }
}

fn project_file(root: &Path, project_id: &str) -> PathBuf {
    root.join("projects").join(format!("{project_id}.json"))
}

fn read_entries(path: &Path, scope: Scope) -> Vec<MemoryEntry> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut entries: Vec<MemoryEntry> = serde_json::from_str(&raw).unwrap_or_default();
    for entry in &mut entries {
        entry.scope = scope;
    }
    entries
}

fn doc_text(entry: &MemoryEntry) -> String {
    format!("{} {}", entry.content, entry.tags.join(" "))
}

fn recent(docs: Vec<(&MemoryEntry, Vec<String>)>, limit: usize) -> Vec<MemoryEntry> {
    let mut entries: Vec<&MemoryEntry> = docs.into_iter().map(|(entry, _)| entry).collect();
    entries.sort_by_key(|a| std::cmp::Reverse(a.updated_at));
    entries.into_iter().take(limit).cloned().collect()
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| token.len() >= 2)
        .map(|token| token.to_lowercase())
        .collect()
}

fn recency_bonus(entry: &MemoryEntry) -> f32 {
    let age_days = now_secs().saturating_sub(entry.updated_at) as f32 / 86_400.0;
    0.5 / (1.0 + age_days)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:04x}", nanos, COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// The id of a project for stores that may be *shared* between checkouts: a
/// memory entry and a session are about the repository, so two clones of one
/// remote name the same one.
pub(crate) fn project_id(cwd: &Path) -> String {
    if let Some(url) = git_remote(cwd) {
        return format!("{:016x}", fnv1a(url.as_bytes()));
    }
    local_project_id(cwd)
}

/// The id of the *local* project: where it sits rather than what it is a clone
/// of. Anything that reaches into the work tree — the shadow snapshot a run's
/// changes are diffed and undone from — has to be keyed by that instead: two
/// clones of one remote are the same repository but not the same files, so a
/// baseline taken in one must not be restorable into the other.
pub(crate) fn local_project_id(cwd: &Path) -> String {
    let root = crate::ecosystem::project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let canonical = root.canonicalize().unwrap_or(root);
    format!("{:016x}", fnv1a(canonical.to_string_lossy().as_bytes()))
}

fn git_remote(cwd: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_mem_{tag}_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn add_search_forget_round_trip() {
        let root = temp_dir("round");
        let cwd = temp_dir("round_proj");
        let store = MemoryStore::open(root.clone(), &cwd);

        let entry = store
            .add(
                "Project uses a microservices architecture",
                Scope::Project,
                vec!["arch".into()],
            )
            .unwrap();
        assert_eq!(
            store.search("architecture", QueryScope::Project, 5).len(),
            1
        );
        assert_eq!(store.search("microservices", QueryScope::All, 5).len(), 1);
        assert!(store
            .search("completely unrelated", QueryScope::Project, 5)
            .is_empty());

        assert!(store.forget(&entry.id).unwrap());
        assert!(store
            .search("architecture", QueryScope::Project, 5)
            .is_empty());
        assert!(!store.forget(&entry.id).unwrap());

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn persists_and_separates_scopes() {
        let root = temp_dir("scope");
        let cwd = temp_dir("scope_proj");
        let store = MemoryStore::open(root.clone(), &cwd);
        store.add("user prefers tabs", Scope::User, vec![]).unwrap();
        store
            .add("project is written in rust", Scope::Project, vec![])
            .unwrap();

        let reopened = MemoryStore::open(root.clone(), &cwd);
        assert_eq!(reopened.list(QueryScope::User, 10).len(), 1);
        assert_eq!(reopened.list(QueryScope::Project, 10).len(), 1);
        assert_eq!(reopened.list(QueryScope::All, 10).len(), 2);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn profile_tracks_tagged_user_entry() {
        let root = temp_dir("profile");
        let cwd = temp_dir("profile_proj");
        let store = MemoryStore::open(root.clone(), &cwd);
        assert!(store.profile().is_none());

        store
            .add(
                "User is a Rust engineer",
                Scope::User,
                vec![PROFILE_TAG.into()],
            )
            .unwrap();
        assert_eq!(store.profile().unwrap().content, "User is a Rust engineer");

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn project_id_is_stable_for_same_path() {
        let cwd = temp_dir("stable");
        assert_eq!(project_id(&cwd), project_id(&cwd));
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn two_clones_of_one_remote_share_an_id_but_not_their_snapshots() {
        let root = temp_dir("clones");
        let clone = |name: &str| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            for args in [
                vec!["init", "--quiet"],
                vec![
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/example/oxide.git",
                ],
            ] {
                let status = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&dir)
                    .args(&args)
                    .output()
                    .unwrap()
                    .status;
                assert!(status.success(), "git {args:?} in {}", dir.display());
            }
            dir
        };
        let one = clone("one");
        let two = clone("two");

        // One repository, so the memory store and the sessions are shared...
        assert_eq!(project_id(&one), project_id(&two));
        // ...but two work trees, so a run's shadow snapshot is not: a baseline
        // taken in one checkout must never be restored into the other.
        assert_ne!(local_project_id(&one), local_project_id(&two));

        // A folder that is not a clone has no remote to be named by, so both
        // ids agree on where it is.
        let plain = temp_dir("clones_plain");
        assert_eq!(project_id(&plain), local_project_id(&plain));

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&plain).ok();
    }
}
