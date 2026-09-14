//! Durable session log. Every model-visible message is appended to an
//! append-only JSONL file under `~/.config/oxide/sessions/<project>/`,
//! so a conversation can be resumed and its model history reconstructed from
//! the log alone.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::dcp::{Compression, DcpState};
use crate::llm::Message;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    pub id: String,
    pub project: String,
    pub cwd: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Record {
    Header(SessionHeader),
    Message(Box<Message>),
    Dcp(Compression),
}

#[derive(Debug, Clone)]
pub struct SessionLog {
    path: PathBuf,
    header: SessionHeader,
}

impl SessionLog {
    pub fn create(cwd: &Path) -> Result<Self> {
        Self::create_in(&project_dir(cwd), cwd)
    }

    pub(crate) fn create_in(dir: &Path, cwd: &Path) -> Result<Self> {
        let id = new_id();
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating session directory {}", dir.display()))?;
        let path = dir.join(format!("{id}.jsonl"));
        let header = SessionHeader {
            id,
            project: crate::memory::project_id(cwd),
            cwd: cwd.display().to_string(),
            created_at: now_secs(),
            name: None,
        };
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("creating session log {}", path.display()))?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&Record::Header(header.clone()))?
        )?;
        Ok(Self { path, header })
    }

    pub fn open(path: PathBuf) -> Result<Self> {
        let header = read_header(&path)?;
        Ok(Self { path, header })
    }

    /// Creates a new session seeded with `messages` (used by `/fork` and
    /// `/clone`). Returns the new log so the caller can continue in it.
    pub fn fork(cwd: &Path, messages: &[Message]) -> Result<Self> {
        let log = Self::create(cwd)?;
        for message in messages {
            log.append(message)?;
        }
        Ok(log)
    }

    pub fn open_id(cwd: &Path, id: &str) -> Result<Self> {
        let path = project_dir(cwd).join(format!("{id}.jsonl"));
        if !path.exists() {
            anyhow::bail!("no session `{id}` for this project");
        }
        Self::open(path)
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

    pub fn id(&self) -> &str {
        &self.header.id
    }

    /// The recorded working directory for this session.
    pub fn cwd(&self) -> &str {
        &self.header.cwd
    }

    /// Sets a human-readable display name for the session and persists it in a
    /// sidecar file (the append-only log header is never rewritten).
    pub fn set_name(&self, name: &str) -> Result<()> {
        let path = self.path.with_extension("name");
        std::fs::write(&path, name)
            .with_context(|| format!("writing session name {}", path.display()))?;
        Ok(())
    }

    /// The session display name, when one was set with `--name` or `/name`.
    pub fn name(&self) -> Option<String> {
        let path = self.path.with_extension("name");
        std::fs::read_to_string(path)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
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

    pub fn append(&self, message: &Message) -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .with_context(|| format!("opening session log {}", self.path.display()))?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&Record::Message(Box::new(message.clone())))?
        )?;
        Ok(())
    }

    pub fn messages(&self) -> Result<Vec<Message>> {
        read_messages(&self.path)
    }

    /// Appends a dynamic-context-pruning compression record.
    pub fn append_dcp(&self, compression: &Compression) -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .with_context(|| format!("opening session log {}", self.path.display()))?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&Record::Dcp(compression.clone()))?
        )?;
        Ok(())
    }

    /// Reconstructs pruning state by replaying compression records.
    pub fn dcp_state(&self) -> DcpState {
        let mut state = DcpState::default();
        let Ok(raw) = std::fs::read_to_string(&self.path) else {
            return state;
        };
        for line in raw.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(Record::Dcp(compression)) = serde_json::from_str::<Record>(line) {
                state.compressions.push(compression);
            }
        }
        state
    }
}

fn project_dir(cwd: &Path) -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("oxide")
        .join("sessions")
        .join(crate::memory::project_id(cwd))
}

fn read_header(path: &Path) -> Result<SessionHeader> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading session log {}", path.display()))?;
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(Record::Header(header)) = serde_json::from_str::<Record>(line) {
            return Ok(header);
        }
    }
    anyhow::bail!("session log {} has no header", path.display())
}

fn read_messages(path: &Path) -> Result<Vec<Message>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading session log {}", path.display()))?;
    let mut messages = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(Record::Message(message)) = serde_json::from_str::<Record>(line) {
            messages.push(*message);
        }
    }
    Ok(messages)
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
    fn dcp_records_round_trip() {
        use crate::dcp::{Compression, Range};

        let dir = temp_dir("dcp");
        let cwd = temp_dir("dcp_proj");
        let log = SessionLog::create_in(&dir, &cwd).unwrap();
        log.append_dcp(&Compression {
            seq: 0,
            ranges: vec![Range { start: 0, end: 1 }],
            summary: "did the first thing".into(),
        })
        .unwrap();
        log.append(&Message::user("still here")).unwrap();

        let reopened = SessionLog::open(log.path().to_path_buf()).unwrap();
        let state = reopened.dcp_state();
        assert_eq!(state.compressions.len(), 1);
        assert_eq!(state.compressions[0].summary, "did the first thing");
        assert_eq!(reopened.messages().unwrap().len(), 1);

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
}
