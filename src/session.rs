//! Durable session log. Every model-visible message is appended to an
//! append-only JSONL file under `~/.config/oxide/sessions/<project>/`,
//! so a conversation can be resumed and its model history reconstructed from
//! the log alone.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
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
        file.sync_data()
            .with_context(|| format!("syncing session log {}", path.display()))?;
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
            let Ok(header) = read_header(&path) else {
                continue;
            };
            let modified_at = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .map(system_time_secs)
                .unwrap_or(header.created_at);
            let scan = scan_messages(&path);
            out.push(SessionSummary {
                id: header.id,
                name: read_name(&path),
                cwd: header.cwd,
                created_at: header.created_at,
                modified_at,
                message_count: scan.message_count,
                preview: scan.preview.unwrap_or_default(),
                path,
            });
        }
        out.sort_by_key(|summary| std::cmp::Reverse(summary.modified_at));
        Ok(out)
    }

    /// Deletes a session file (and its name sidecar) for a project.
    pub fn delete(cwd: &Path, id: &str) -> Result<()> {
        let path = project_dir(cwd).join(format!("{id}.jsonl"));
        if !path.exists() {
            anyhow::bail!("no session `{id}` for this project");
        }
        remove_file(&path)?;
        let _ = remove_file(&path.with_extension("name"));
        Ok(())
    }

    /// Renames a session by id, writing the display-name sidecar.
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
        read_name(&self.path)
    }

    /// Lightweight picker metadata for this session.
    pub fn summary(&self) -> Result<SessionSummary> {
        let scan = scan_messages(&self.path);
        let modified_at = std::fs::metadata(&self.path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .map(system_time_secs)
            .unwrap_or(self.header.created_at);
        Ok(SessionSummary {
            id: self.header.id.clone(),
            name: self.name(),
            cwd: self.header.cwd.clone(),
            created_at: self.header.created_at,
            modified_at,
            message_count: scan.message_count,
            preview: scan.preview.unwrap_or_default(),
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

    pub fn append(&self, message: &Message) -> Result<()> {
        let line = serde_json::to_string(&Record::Message(Box::new(message.clone())))?;
        append_line(&self.path, &line)
    }

    pub fn messages(&self) -> Result<Vec<Message>> {
        read_messages(&self.path)
    }

    /// Appends a dynamic-context-pruning compression record.
    pub fn append_dcp(&self, compression: &Compression) -> Result<()> {
        let line = serde_json::to_string(&Record::Dcp(compression.clone()))?;
        append_line(&self.path, &line)
    }

    /// Rewrites the session file with a compacted message history, preserving
    /// the header and id. Used by `oxide sessions compact`.
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
            serde_json::to_string(&Record::Header(self.header.clone()))?
        )?;
        for message in messages {
            writeln!(
                file,
                "{}",
                serde_json::to_string(&Record::Message(Box::new(message.clone())))?
            )?;
        }
        file.sync_data()
            .with_context(|| format!("syncing session log {}", tmp.display()))?;
        drop(file);
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("replacing {}", self.path.display()))?;
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

fn sessions_root() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("oxide")
        .join("sessions")
}

fn project_dir(cwd: &Path) -> PathBuf {
    sessions_root().join(crate::memory::project_id(cwd))
}

fn read_name(path: &Path) -> Option<String> {
    std::fs::read_to_string(path.with_extension("name"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_header(path: &Path) -> Result<SessionHeader> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("reading session log {}", path.display()))?;
    for line in BufReader::new(file).lines() {
        let line = line.with_context(|| format!("reading session log {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(Record::Header(header)) = serde_json::from_str::<Record>(&line) {
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

struct MessageScan {
    message_count: usize,
    preview: Option<String>,
}

/// Scans a session log for its message count and first user-message preview
/// without deserializing every message body.
fn scan_messages(path: &Path) -> MessageScan {
    let mut scan = MessageScan {
        message_count: 0,
        preview: None,
    };
    let Ok(file) = std::fs::File::open(path) else {
        return scan;
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("\"kind\":\"message\"") {
            continue;
        }
        scan.message_count += 1;
        if scan.preview.is_none() {
            if let Ok(Record::Message(message)) = serde_json::from_str::<Record>(&line) {
                if message.role == "user" {
                    if let Some(text) = message.display() {
                        scan.preview = Some(preview_text(&text));
                    }
                }
            }
        }
    }
    scan
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

fn now_secs() -> u64 {
    system_time_secs(SystemTime::now())
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

/// Appends one JSONL record and flushes it to disk. If a previous write was
/// interrupted mid-record, the trailing partial line is terminated first so the
/// new record starts on a fresh line instead of being concatenated into it.
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
    fn preview_truncates_long_first_lines() {
        let long = "x".repeat(100);
        let preview = preview_text(&format!("  {long}\nsecond line"));
        assert_eq!(preview.chars().count(), 81);
        assert!(preview.ends_with('…'));
        assert!(preview.starts_with('x'));
    }

    #[test]
    fn append_recovers_from_partial_trailing_line() {
        let dir = temp_dir("partial");
        let cwd = temp_dir("partial_proj");
        let log = SessionLog::create_in(&dir, &cwd).unwrap();
        log.append(&Message::user("one")).unwrap();

        // Simulate a crash that left a partial record with no trailing newline.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(log.path())
            .unwrap();
        write!(
            file,
            "{{\"kind\":\"message\",\"role\":\"user\",\"content\":\"bro"
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
}
