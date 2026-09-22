//! Pi-compatible session storage.
//!
//! A session is an append-only JSONL file whose first line is a `session`
//! header and whose remaining lines are typed entries linked by
//! `id`/`parentId`. The entries form a tree: the last entry is the active leaf,
//! appending a message adds a child of the leaf, and branching moves the leaf
//! back to an earlier entry so the next append starts a new branch. Compaction
//! and branch-summary entries summarize older or abandoned context, and the
//! model sees the branch from the leaf to the root with the latest compaction
//! applied.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::compact::{CompactionDetails, UsageRecord};
use crate::llm::Message;

pub const SESSION_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    pub version: u32,
    pub id: String,
    pub timestamp: String,
    pub cwd: String,
    #[serde(
        default,
        rename = "parentSession",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_session: Option<String>,
}

/// Cumulative token and cost totals for the footer, matching Pi's aggregation.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UsageTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    pub cache_hit_rate: Option<f64>,
}

/// Lightweight metadata for one persisted session, used by the session picker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub name: Option<String>,
    pub cwd: String,
    pub created_at: u64,
    pub modified_at: u64,
    pub message_count: usize,
    pub preview: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Entry {
    Session(SessionHeader),
    Message(MessageEntry),
    Compaction(CompactionEntry),
    BranchSummary(BranchSummaryEntry),
    SessionInfo(SessionInfoEntry),
    ModelChange(ModelChangeEntry),
    ThinkingLevelChange(ThinkingLevelEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub message: Box<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub summary: String,
    #[serde(rename = "firstKeptEntryId")]
    pub first_kept_entry_id: String,
    #[serde(rename = "tokensBefore")]
    pub tokens_before: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<CompactionDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSummaryEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(rename = "fromId", default, skip_serializing_if = "Option::is_none")]
    pub from_id: Option<String>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<CompactionDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfoEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelChangeEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub provider: String,
    #[serde(rename = "modelId")]
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingLevelEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(rename = "thinkingLevel")]
    pub thinking_level: String,
}

impl Entry {
    pub fn id(&self) -> &str {
        match self {
            Entry::Session(header) => &header.id,
            Entry::Message(entry) => &entry.id,
            Entry::Compaction(entry) => &entry.id,
            Entry::BranchSummary(entry) => &entry.id,
            Entry::SessionInfo(entry) => &entry.id,
            Entry::ModelChange(entry) => &entry.id,
            Entry::ThinkingLevelChange(entry) => &entry.id,
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        match self {
            Entry::Session(_) => None,
            Entry::Message(entry) => entry.parent_id.as_deref(),
            Entry::Compaction(entry) => entry.parent_id.as_deref(),
            Entry::BranchSummary(entry) => entry.parent_id.as_deref(),
            Entry::SessionInfo(entry) => entry.parent_id.as_deref(),
            Entry::ModelChange(entry) => entry.parent_id.as_deref(),
            Entry::ThinkingLevelChange(entry) => entry.parent_id.as_deref(),
        }
    }

    /// Messages this entry contributes to the model context, in Pi's shape:
    /// stored messages pass through, compactions and branch summaries become
    /// prefixed user messages, and bookkeeping entries contribute nothing.
    pub fn context_messages(&self) -> Vec<Message> {
        match self {
            Entry::Message(entry) => vec![(*entry.message).clone()],
            Entry::Compaction(entry) => vec![Message::user(format!(
                "[conversation summary]\n{}",
                entry.summary
            ))],
            Entry::BranchSummary(entry) => vec![Message::user(format!(
                "[branch summary]\n{}",
                entry.summary
            ))],
            _ => Vec::new(),
        }
    }

    fn is_system_message(&self) -> bool {
        matches!(self, Entry::Message(entry) if entry.message.role == "system")
    }
}

#[derive(Debug, Default)]
struct SessionState {
    entries: Vec<Entry>,
    by_id: HashMap<String, usize>,
    leaf_id: Option<String>,
}

/// An append-only session tree.
#[derive(Debug, Clone)]
pub struct SessionLog {
    path: PathBuf,
    header: SessionHeader,
    state: Arc<Mutex<SessionState>>,
}

impl SessionLog {
    pub fn create(cwd: &Path) -> Result<Self> {
        Self::create_in(&project_dir(cwd), cwd)
    }

    pub(crate) fn create_in(dir: &Path, cwd: &Path) -> Result<Self> {
        let id = new_id();
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating session directory {}", dir.display()))?;
        let path = dir.join(format!("{}.jsonl", session_file_stem(&id)));
        let header = SessionHeader {
            version: SESSION_VERSION,
            id,
            timestamp: now_iso(),
            cwd: cwd.display().to_string(),
            parent_session: None,
        };
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("creating session log {}", path.display()))?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&Entry::Session(header.clone()))?
        )?;
        file.sync_data()
            .with_context(|| format!("syncing session log {}", path.display()))?;
        Ok(Self {
            path,
            header,
            state: Arc::new(Mutex::new(SessionState::default())),
        })
    }

    pub fn open(path: PathBuf) -> Result<Self> {
        let (header, state) = read_session(&path)?;
        Ok(Self {
            path,
            header,
            state: Arc::new(Mutex::new(state)),
        })
    }

    /// Creates a new session seeded with a linear chain of `messages`. Used by
    /// `/fork` and `/clone` (Pi forks into a new session file).
    pub fn fork(cwd: &Path, messages: &[Message]) -> Result<Self> {
        let log = Self::create(cwd)?;
        for message in messages {
            log.append(message)?;
        }
        Ok(log)
    }

    pub fn open_id(cwd: &Path, id: &str) -> Result<Self> {
        let dir = project_dir(cwd);
        let direct = dir.join(format!("{id}.jsonl"));
        if direct.exists() {
            return Self::open(direct);
        }
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            if let Ok((header, _)) = read_session(&path) {
                if header.id == id || path.file_stem().is_some_and(|stem| stem == id) {
                    return Self::open(path);
                }
            }
        }
        anyhow::bail!("no session `{id}` for this project")
    }

    pub fn latest(cwd: &Path) -> Option<Self> {
        Self::latest_in(&project_dir(cwd))
    }

    pub(crate) fn latest_in(dir: &Path) -> Option<Self> {
        let mut files: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
            .filter_map(|entry| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                Some((modified, entry.path()))
            })
            .collect();
        files.sort_by_key(|a| std::cmp::Reverse(a.0));
        let (_, path) = files.into_iter().next()?;
        Self::open(path).ok()
    }

    /// Lists persisted sessions for a project, newest first.
    pub fn list(cwd: &Path) -> Result<Vec<SessionSummary>> {
        Self::list_in(&project_dir(cwd))
    }

    /// Lists persisted sessions across every project, newest first.
    pub fn list_all() -> Result<Vec<SessionSummary>> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(sessions_root()) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Ok(mut sessions) = Self::list_in(&entry.path()) {
                        out.append(&mut sessions);
                    }
                }
            }
        }
        out.sort_by_key(|summary| std::cmp::Reverse(summary.modified_at));
        Ok(out)
    }

    pub(crate) fn list_in(dir: &Path) -> Result<Vec<SessionSummary>> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(err) => {
                return Err(err).with_context(|| format!("listing sessions in {}", dir.display()))
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Ok((header, state)) = read_session(&path) else {
                continue;
            };
            let modified_at = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .map(system_time_secs)
                .unwrap_or_else(|| parse_iso(&header.timestamp).unwrap_or(0));
            let name = latest_name(&state.entries);
            let path_desc = leaf_path(&state);
            let message_count = path_desc
                .iter()
                .filter(|entry| matches!(entry, Entry::Message(_)))
                .count();
            let preview = path_desc
                .iter()
                .find_map(|entry| match entry {
                    Entry::Message(message) if message.message.role == "user" => {
                        message.message.display().map(|text| preview_text(&text))
                    }
                    _ => None,
                })
                .unwrap_or_default();
            out.push(SessionSummary {
                id: header.id,
                name,
                cwd: header.cwd,
                created_at: parse_iso(&header.timestamp).unwrap_or(0),
                modified_at,
                message_count,
                preview,
                path,
            });
        }
        out.sort_by_key(|summary| std::cmp::Reverse(summary.modified_at));
        Ok(out)
    }

    /// Deletes a session file for a project.
    pub fn delete(cwd: &Path, id: &str) -> Result<()> {
        let log = Self::open_id(cwd, id)?;
        remove_file(&log.path)
    }

    /// Renames a session by appending a `session_info` entry.
    pub fn rename(cwd: &Path, id: &str, name: &str) -> Result<()> {
        Self::open_id(cwd, id)?.set_name(name)
    }

    pub fn id(&self) -> &str {
        &self.header.id
    }

    /// The recorded working directory for this session.
    pub fn cwd(&self) -> &str {
        &self.header.cwd
    }

    /// Sets a human-readable display name via a `session_info` entry.
    pub fn set_name(&self, name: &str) -> Result<()> {
        let entry = Entry::SessionInfo(SessionInfoEntry {
            id: new_id(),
            parent_id: self.leaf_id(),
            timestamp: now_iso(),
            name: name.to_string(),
        });
        self.append_entry(entry)?;
        Ok(())
    }

    /// The session display name from the latest `session_info` entry.
    pub fn name(&self) -> Option<String> {
        latest_name(&self.state().entries)
    }

    /// Lightweight picker metadata for this session.
    pub fn summary(&self) -> Result<SessionSummary> {
        let state = self.state();
        let modified_at = std::fs::metadata(&self.path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .map(system_time_secs)
            .unwrap_or_else(|| parse_iso(&self.header.timestamp).unwrap_or(0));
        let path_desc = leaf_path(&state);
        let message_count = path_desc
            .iter()
            .filter(|entry| matches!(entry, Entry::Message(_)))
            .count();
        let preview = path_desc
            .iter()
            .find_map(|entry| match entry {
                Entry::Message(message) if message.message.role == "user" => {
                    message.message.display().map(|text| preview_text(&text))
                }
                _ => None,
            })
            .unwrap_or_default();
        Ok(SessionSummary {
            id: self.header.id.clone(),
            name: latest_name(&state.entries),
            cwd: self.header.cwd.clone(),
            created_at: parse_iso(&self.header.timestamp).unwrap_or(0),
            modified_at,
            message_count,
            preview,
            path: self.path.clone(),
        })
    }

    /// Opens a session log at an explicit path, accepting either a full path or
    /// an id resolved against the project's session directory.
    pub fn open_ref(cwd: &Path, reference: &str) -> Result<Self> {
        let path = Path::new(reference);
        if path.exists() {
            return Self::open(path.to_path_buf());
        }
        Self::open_id(cwd, reference)
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends a message as a child of the current leaf and returns its entry id.
    pub fn append(&self, message: &Message) -> Result<String> {
        self.append_with_usage(message, None)
    }

    /// Appends a message with the provider usage that produced it, so token
    /// totals survive a resume like Pi's per-message usage.
    pub fn append_with_usage(
        &self,
        message: &Message,
        usage: Option<UsageRecord>,
    ) -> Result<String> {
        let entry = Entry::Message(MessageEntry {
            id: new_id(),
            parent_id: self.leaf_id(),
            timestamp: now_iso(),
            message: Box::new(message.clone()),
            usage,
        });
        self.append_entry(entry)
    }

    /// Cumulative usage across the whole session, including assistant turns and
    /// summary generation (Pi's footer totals), plus the latest request's cache
    /// hit rate.
    pub fn usage_totals(&self) -> UsageTotals {
        let mut totals = UsageTotals::default();
        let state = self.state();
        for entry in &state.entries {
            let usage = match entry {
                Entry::Message(entry) => entry.usage,
                Entry::Compaction(entry) => entry.usage,
                Entry::BranchSummary(entry) => entry.usage,
                _ => None,
            };
            if let Some(usage) = usage {
                totals.input += usage.input;
                totals.output += usage.output;
                totals.cache_read += usage.cache_read;
                totals.cache_write += usage.cache_write;
                totals.cost += usage.cost;
                let prompt = usage.input + usage.cache_read + usage.cache_write;
                if prompt > 0 && usage.cache_read + usage.cache_write > 0 {
                    totals.cache_hit_rate = Some(usage.cache_read as f64 / prompt as f64 * 100.0);
                }
            }
        }
        totals
    }

    /// Appends a compaction entry and returns its id.
    #[allow(clippy::too_many_arguments)]
    pub fn append_compaction(
        &self,
        summary: impl Into<String>,
        first_kept_entry_id: impl Into<String>,
        tokens_before: u64,
        details: Option<CompactionDetails>,
        usage: Option<UsageRecord>,
    ) -> Result<String> {
        let entry = Entry::Compaction(CompactionEntry {
            id: new_id(),
            parent_id: self.leaf_id(),
            timestamp: now_iso(),
            summary: summary.into(),
            first_kept_entry_id: first_kept_entry_id.into(),
            tokens_before,
            usage,
            details,
        });
        self.append_entry(entry)
    }

    /// Appends a branch-summary entry after moving the leaf, capturing context
    /// from the abandoned path. Returns the new branch-summary entry id.
    pub fn branch_with_summary(
        &self,
        branch_from: Option<&str>,
        summary: impl Into<String>,
        details: Option<CompactionDetails>,
        usage: Option<UsageRecord>,
    ) -> Result<String> {
        let from_id = self.leaf_id();
        let entry = Entry::BranchSummary(BranchSummaryEntry {
            id: new_id(),
            parent_id: branch_from.map(str::to_string),
            timestamp: now_iso(),
            from_id,
            summary: summary.into(),
            usage,
            details,
        });
        self.append_entry(entry)
    }

    /// Records a model switch on the active path.
    pub fn append_model_change(&self, provider: &str, model_id: &str) -> Result<String> {
        let entry = Entry::ModelChange(ModelChangeEntry {
            id: new_id(),
            parent_id: self.leaf_id(),
            timestamp: now_iso(),
            provider: provider.to_string(),
            model_id: model_id.to_string(),
        });
        self.append_entry(entry)
    }

    /// Records a thinking-level change on the active path.
    pub fn append_thinking_level(&self, level: &str) -> Result<String> {
        let entry = Entry::ThinkingLevelChange(ThinkingLevelEntry {
            id: new_id(),
            parent_id: self.leaf_id(),
            timestamp: now_iso(),
            thinking_level: level.to_string(),
        });
        self.append_entry(entry)
    }

    /// The messages the model sees: the leaf path with the latest compaction
    /// applied.
    pub fn messages(&self) -> Result<Vec<Message>> {
        Ok(self
            .context()
            .iter()
            .flat_map(Entry::context_messages)
            .collect())
    }

    /// Context entries (the leaf path with compaction applied).
    pub fn context(&self) -> Vec<Entry> {
        let state = self.state();
        context_entries(&state.entries, state.leaf_id.as_deref(), &state.by_id)
    }

    /// Entry ids aligned with [`Self::messages`], used to anchor compaction.
    pub fn context_ids(&self) -> Vec<String> {
        self.context()
            .iter()
            .map(|entry| entry.id().to_string())
            .collect()
    }

    /// Every entry in append order (excluding the header).
    pub fn entries(&self) -> Vec<Entry> {
        self.state()
            .entries
            .iter()
            .filter(|entry| !matches!(entry, Entry::Session(_)))
            .cloned()
            .collect()
    }

    /// The current leaf entry id.
    pub fn leaf_id(&self) -> Option<String> {
        self.state().leaf_id.clone()
    }

    /// Replaces the on-disk entry list with a linear sequence of messages. Used
    /// by `oxide sessions compact` after summarizing a session in place.
    pub fn rewrite(&self, messages: &[Message]) -> Result<()> {
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        let tmp = dir.join(format!(".{}.tmp", self.header.id));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("writing session log {}", tmp.display()))?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&Entry::Session(self.header.clone()))?
        )?;
        let mut parent: Option<String> = None;
        let mut entries = Vec::new();
        for message in messages {
            let id = new_id();
            let entry = Entry::Message(MessageEntry {
                id: id.clone(),
                parent_id: parent.clone(),
                timestamp: now_iso(),
                message: Box::new(message.clone()),
                usage: None,
            });
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
            parent = Some(id);
            entries.push(entry);
        }
        file.sync_data()
            .with_context(|| format!("syncing session log {}", tmp.display()))?;
        drop(file);
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("replacing {}", self.path.display()))?;

        let mut state = self.state();
        state.by_id.clear();
        state.leaf_id = parent;
        for (index, entry) in entries.iter().enumerate() {
            state.by_id.insert(entry.id().to_string(), index);
        }
        state.entries = entries;
        Ok(())
    }

    fn append_entry(&self, entry: Entry) -> Result<String> {
        let id = entry.id().to_string();
        let line = serde_json::to_string(&entry)?;
        append_line(&self.path, &line)?;
        let mut state = self.state();
        let index = state.entries.len();
        state.by_id.insert(id.clone(), index);
        state.leaf_id = Some(id.clone());
        state.entries.push(entry);
        Ok(id)
    }

    fn state(&self) -> MutexGuard<'_, SessionState> {
        self.state.lock().unwrap_or_else(|err| err.into_inner())
    }
}

/// The leaf path (last entry is the leaf) with the latest compaction applied.
fn context_entries(
    entries: &[Entry],
    leaf_id: Option<&str>,
    by_id: &HashMap<String, usize>,
) -> Vec<Entry> {
    let path = leaf_path_with(entries, leaf_id, by_id);
    let Some(compaction_pos) = path
        .iter()
        .rposition(|entry| matches!(entry, Entry::Compaction(_)))
    else {
        return path;
    };
    let Some(Entry::Compaction(compaction)) = path.get(compaction_pos) else {
        return path;
    };
    let mut out = vec![path[compaction_pos].clone()];
    let mut found_first_kept = false;
    for entry in &path[..compaction_pos] {
        if entry.id() == compaction.first_kept_entry_id {
            found_first_kept = true;
        }
        if found_first_kept && !entry.is_system_message() {
            out.push(entry.clone());
        }
    }
    out.extend_from_slice(&path[compaction_pos + 1..]);
    out
}

fn leaf_path(state: &SessionState) -> Vec<Entry> {
    leaf_path_with(&state.entries, state.leaf_id.as_deref(), &state.by_id)
}

fn leaf_path_with(
    entries: &[Entry],
    leaf_id: Option<&str>,
    by_id: &HashMap<String, usize>,
) -> Vec<Entry> {
    let leaf = leaf_id
        .and_then(|id| by_id.get(id).copied())
        .or_else(|| entries.len().checked_sub(1));
    let Some(mut current) = leaf else {
        return Vec::new();
    };
    let mut path = Vec::new();
    loop {
        path.push(entries[current].clone());
        let Some(parent) = entries[current]
            .parent_id()
            .and_then(|id| by_id.get(id).copied())
        else {
            break;
        };
        current = parent;
    }
    path.reverse();
    path
}

fn latest_name(entries: &[Entry]) -> Option<String> {
    entries.iter().rev().find_map(|entry| match entry {
        Entry::SessionInfo(info) => Some(info.name.clone()),
        _ => None,
    })
}

fn sessions_root() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("oxide")
        .join("sessions")
}

fn project_dir(cwd: &Path) -> PathBuf {
    sessions_root().join(crate::memory::project_id(cwd))
}

fn read_session(path: &Path) -> Result<(SessionHeader, SessionState)> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("reading session log {}", path.display()))?;
    let mut header = None;
    let mut state = SessionState::default();
    for line in BufReader::new(file).lines() {
        let line = line.with_context(|| format!("reading session log {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Entry>(&line) else {
            continue;
        };
        match entry {
            Entry::Session(value) => header = Some(value),
            other => {
                state
                    .by_id
                    .insert(other.id().to_string(), state.entries.len());
                state.leaf_id = Some(other.id().to_string());
                state.entries.push(other);
            }
        }
    }
    let header = header.with_context(|| format!("session log {} has no header", path.display()))?;
    Ok((header, state))
}

fn preview_text(text: &str) -> String {
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut preview: String = first.chars().take(80).collect();
    if first.chars().count() > 80 {
        preview.push('…');
    }
    preview
}

fn system_time_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn remove_file(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if let Ok(status) = std::process::Command::new("trash").arg(path).status() {
        if status.success() {
            return Ok(());
        }
    }
    std::fs::remove_file(path).with_context(|| format!("deleting {}", path.display()))
}

/// Appends one JSONL record and flushes it to disk. A partial trailing line
/// from an interrupted write is terminated first so the record starts fresh.
fn append_line(path: &Path, line: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening session log {}", path.display()))?;

    ensure_terminated_line(&mut file)?;
    writeln!(file, "{line}")
        .with_context(|| format!("appending to session log {}", path.display()))?;
    file.sync_data()
        .with_context(|| format!("syncing session log {}", path.display()))?;
    Ok(())
}

fn ensure_terminated_line(file: &mut std::fs::File) -> Result<()> {
    let len = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if len == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::Start(len - 1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    if last[0] != b'\n' {
        file.write_all(b"\n")?;
    }
    Ok(())
}

/// Pi-style short entry id: six hex characters derived from time and a counter.
fn new_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let value = nanos
        .rotate_left(17)
        .wrapping_add(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    format!("{:08x}", value & 0xffff_ffff)
}

fn session_file_stem(id: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("{millis}_{id}")
}

fn now_iso() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    format_iso(millis)
}

fn format_iso(millis: u64) -> String {
    let secs = (millis / 1000) as i64;
    let ms = (millis % 1000) as u32;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
}

/// Howard Hinnant's `civil_from_days`, for a dependency-free UTC timestamp.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Parses our own ISO-8601 output back to Unix seconds (used for sorting).
fn parse_iso(text: &str) -> Option<u64> {
    let (date, rest) = text.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let time = rest.trim_end_matches('Z');
    let (clock, _frac) = time.split_once('.').unwrap_or((time, ""));
    let mut clock_parts = clock.split(':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let second: i64 = clock_parts.next()?.parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3600 + minute * 60 + second) as u64)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_sess_{tag}_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn append_and_reload_round_trip() {
        let dir = temp_dir("round");
        let cwd = temp_dir("round_proj");
        let log = SessionLog::create_in(&dir, &cwd).unwrap();
        log.append(&Message::user("hello")).unwrap();
        log.append(&Message::assistant("hi there", vec![])).unwrap();

        let reopened = SessionLog::open(log.path().to_path_buf()).unwrap();
        assert_eq!(reopened.id(), log.id());
        let messages = reopened.messages().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].display().as_deref(), Some("hello"));
        assert_eq!(messages[1].display().as_deref(), Some("hi there"));

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn entries_form_a_tree_with_parent_links() {
        let dir = temp_dir("tree");
        let cwd = temp_dir("tree_proj");
        let log = SessionLog::create_in(&dir, &cwd).unwrap();
        let first = log.append(&Message::user("one")).unwrap();
        let second = log.append(&Message::assistant("two", vec![])).unwrap();
        let entries = log.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].parent_id(), None);
        assert_eq!(entries[1].parent_id(), Some(first.as_str()));
        assert_eq!(log.leaf_id().as_deref(), Some(second.as_str()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn compaction_replaces_older_entries() {
        let dir = temp_dir("compact");
        let cwd = temp_dir("compact_proj");
        let log = SessionLog::create_in(&dir, &cwd).unwrap();
        log.append(&Message::user("one")).unwrap();
        log.append(&Message::assistant("two", vec![])).unwrap();
        let kept = log.append(&Message::user("three")).unwrap();
        log.append_compaction("summary", kept.clone(), 10, None, None)
            .unwrap();
        log.append(&Message::user("four")).unwrap();

        let messages = log.messages().unwrap();
        let texts: Vec<String> = messages.iter().filter_map(Message::display).collect();
        assert_eq!(texts[0], "[conversation summary]\nsummary");
        assert!(texts.contains(&"three".to_string()));
        assert!(texts.contains(&"four".to_string()));
        assert!(!texts.contains(&"one".to_string()));

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn branch_with_summary_starts_a_new_branch() {
        let dir = temp_dir("branch");
        let cwd = temp_dir("branch_proj");
        let log = SessionLog::create_in(&dir, &cwd).unwrap();
        let first = log.append(&Message::user("one")).unwrap();
        log.append(&Message::assistant("two", vec![])).unwrap();
        log.branch_with_summary(Some(&first), "abandoned", None, None)
            .unwrap();
        log.append(&Message::user("redo")).unwrap();

        let messages = log.messages().unwrap();
        let texts: Vec<String> = messages.iter().filter_map(Message::display).collect();
        assert_eq!(
            texts,
            vec![
                "one".to_string(),
                "[branch summary]\nabandoned".to_string(),
                "redo".to_string()
            ]
        );

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn usage_totals_sum_assistant_and_summary_usage() {
        let dir = temp_dir("usage");
        let cwd = temp_dir("usage_proj");
        let log = SessionLog::create_in(&dir, &cwd).unwrap();
        log.append_with_usage(
            &Message::assistant("hi", vec![]),
            Some(UsageRecord {
                input: 100,
                output: 20,
                cache_read: 40,
                cache_write: 10,
                cost: 0.5,
            }),
        )
        .unwrap();
        let kept = log.leaf_id().unwrap();
        log.append_compaction(
            "summary",
            kept,
            10,
            None,
            Some(UsageRecord {
                input: 30,
                output: 5,
                ..Default::default()
            }),
        )
        .unwrap();
        log.branch_with_summary(
            None,
            "branch",
            None,
            Some(UsageRecord {
                input: 7,
                output: 3,
                ..Default::default()
            }),
        )
        .unwrap();
        let totals = log.usage_totals();
        assert_eq!((totals.input, totals.output), (137, 28));
        assert_eq!(totals.cache_read, 40);
        assert_eq!(totals.cache_write, 10);
        assert!((totals.cost - 0.5).abs() < 1e-9);
        let hit = totals.cache_hit_rate.unwrap();
        assert!((hit - 40.0 / 150.0 * 100.0).abs() < 1e-9);

        let reopened = SessionLog::open(log.path().to_path_buf()).unwrap();
        let totals = reopened.usage_totals();
        assert_eq!((totals.input, totals.output), (137, 28));
        assert_eq!(totals.cache_read, 40);

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn session_name_round_trips_as_entry() {
        let dir = temp_dir("name");
        let cwd = temp_dir("name_proj");
        let log = SessionLog::create_in(&dir, &cwd).unwrap();
        log.append(&Message::user("hi")).unwrap();
        log.set_name("my task").unwrap();
        assert_eq!(log.name().as_deref(), Some("my task"));
        let reopened = SessionLog::open(log.path().to_path_buf()).unwrap();
        assert_eq!(reopened.name().as_deref(), Some("my task"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn latest_picks_most_recent() {
        let dir = temp_dir("latest");
        let cwd = temp_dir("latest_proj");
        let first = SessionLog::create_in(&dir, &cwd).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = SessionLog::create_in(&dir, &cwd).unwrap();

        let latest = SessionLog::latest_in(&dir).unwrap();
        assert_eq!(latest.id(), second.id());
        assert_ne!(first.id(), second.id());

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn fork_copies_messages_into_a_new_session() {
        let dir = temp_dir("fork");
        let cwd = temp_dir("fork_proj");
        let parent = SessionLog::create_in(&dir, &cwd).unwrap();
        parent.append(&Message::user("one")).unwrap();
        parent.append(&Message::assistant("two", vec![])).unwrap();
        parent.append(&Message::user("three")).unwrap();

        let messages = parent.messages().unwrap();
        let fork = SessionLog::fork(&cwd, &messages[..2]).unwrap();
        assert_ne!(fork.id(), parent.id());
        let copied = fork.messages().unwrap();
        assert_eq!(copied.len(), 2);
        assert_eq!(copied[0].display().as_deref(), Some("one"));
        assert_eq!(copied[1].display().as_deref(), Some("two"));

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn list_summarizes_messages_and_preview() {
        let dir = temp_dir("list");
        let cwd = temp_dir("list_proj");
        let log = SessionLog::create_in(&dir, &cwd).unwrap();
        log.append(&Message::user("first user message")).unwrap();
        log.append(&Message::assistant("reply", vec![])).unwrap();

        let sessions = SessionLog::list_in(&dir).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, log.id());
        assert_eq!(sessions[0].message_count, 2);
        assert_eq!(sessions[0].preview, "first user message");

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn append_recovers_from_partial_trailing_line() {
        let dir = temp_dir("partial");
        let cwd = temp_dir("partial_proj");
        let log = SessionLog::create_in(&dir, &cwd).unwrap();
        log.append(&Message::user("one")).unwrap();

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(log.path())
            .unwrap();
        write!(
            file,
            "{{\"type\":\"message\",\"id\":\"deadbeef\",\"parentId\":null,\"timestamp\":\"x\",\"message\":"
        )
        .unwrap();

        log.append(&Message::assistant("two", vec![])).unwrap();

        let messages = log.messages().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].display().as_deref(), Some("one"));
        assert_eq!(messages[1].display().as_deref(), Some("two"));

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn iso_round_trips_through_parse() {
        let iso = format_iso(1_733_234_400_000);
        assert_eq!(iso, "2024-12-03T14:00:00.000Z");
        assert_eq!(parse_iso(&iso), Some(1_733_234_400));
    }
}
