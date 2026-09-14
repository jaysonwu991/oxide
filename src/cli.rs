//! Pi-compatible non-interactive output modes and CLI argument helpers.
//!
//! `--mode json` emits every agent event as a JSON line and `--mode rpc`
//! additionally accepts JSONL input on stdin. Both use LF-delimited framing,
//! matching Pi's event-stream contract.

use crate::agent::AgentEvent;
use crate::session::SessionLog;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Non-interactive output mode selected by `--mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Print,
    Json,
}

/// Expands `@path` file arguments into an initial prompt prefix, mirroring
/// Pi's `pi @file "message"` form. Attachments (images/PDFs) are returned
/// separately so they can be sent as media parts.
pub struct FileArgs {
    pub text: String,
    pub attachments: Vec<PathBuf>,
}

pub fn expand_file_args(cwd: &Path, args: &[String]) -> Result<FileArgs> {
    let mut text = String::new();
    let mut attachments = Vec::new();
    for arg in args {
        let Some(path) = arg.strip_prefix('@') else {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(arg);
            continue;
        };
        let full = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            cwd.join(path)
        };
        if crate::media::is_attachment_path(&full) {
            attachments.push(full);
        } else {
            let content = std::fs::read_to_string(&full)
                .with_context(|| format!("reading @{}", full.display()))?;
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&format!("--- {} ---\n{}", path, content));
        }
    }
    Ok(FileArgs { text, attachments })
}

/// A tool allow/deny filter derived from `--tools` / `--exclude-tools`.
#[derive(Debug, Clone, Default)]
pub struct ToolFilter {
    allow: Option<Vec<String>>,
    exclude: Vec<String>,
}

impl ToolFilter {
    pub fn new(allow: Option<String>, exclude: Option<String>) -> Self {
        let split = |value: Option<String>| {
            value
                .map(|list| {
                    list.split(',')
                        .map(|name| name.trim().to_string())
                        .filter(|name| !name.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        Self {
            allow: allow
                .map(|list| split(Some(list)))
                .filter(|v| !v.is_empty()),
            exclude: split(exclude),
        }
    }

    /// True when the filter can exclude at least one tool, so callers can skip
    /// filtering entirely for the common unrestricted case.
    pub fn is_restrictive(&self) -> bool {
        self.allow.is_some() || !self.exclude.is_empty()
    }

    pub fn permits(&self, name: &str) -> bool {
        let canonical = crate::tools::canonical_tool_name(name);
        if self
            .exclude
            .iter()
            .any(|rule| matches_tool(rule, name, canonical))
        {
            return false;
        }
        match &self.allow {
            Some(allow) => allow.iter().any(|rule| matches_tool(rule, name, canonical)),
            None => true,
        }
    }
}

fn matches_tool(rule: &str, raw: &str, canonical: &str) -> bool {
    rule == raw || rule == canonical || crate::tools::canonical_tool_name(rule) == canonical
}

/// JSON line emitted at the start of a session, matching Pi's header shape.
pub fn session_header(log: &SessionLog) -> Value {
    json!({
        "type": "session",
        "version": 3,
        "id": log.id(),
        "cwd": log.cwd(),
    })
}

/// Serializes one agent event as a Pi-compatible JSON line. Returns `None` for
/// events that have no wire representation (there are none today, but this
/// keeps the mapping explicit).
pub fn event_json(event: &AgentEvent) -> Option<Value> {
    let value = match event {
        AgentEvent::Thought { .. } => json!({ "type": "thinking" }),
        AgentEvent::Text(delta) => json!({
            "type": "message_update",
            "assistantMessageEvent": { "type": "text_delta", "delta": delta }
        }),
        AgentEvent::ToolCall { name, args } => json!({
            "type": "tool_call",
            "toolName": name,
            "arguments": args,
        }),
        AgentEvent::ToolProgress { name, chunk } => json!({
            "type": "tool_execution_update",
            "toolName": name,
            "partialResult": chunk,
        }),
        AgentEvent::ToolResult { name, output, .. } => json!({
            "type": "tool_execution_end",
            "toolName": name,
            "result": output,
            "isError": output.starts_with("error:"),
        }),
        AgentEvent::Usage { input, output } => json!({
            "type": "usage",
            "usage": { "input": input, "output": output }
        }),
        AgentEvent::Error(message) => json!({ "type": "error", "message": message }),
        AgentEvent::Finished(messages) => json!({
            "type": "agent_end",
            "messages": messages.iter().map(message_json).collect::<Vec<_>>(),
        }),
    };
    Some(value)
}

fn message_json(message: &crate::llm::Message) -> Value {
    json!({
        "role": message.role,
        "content": message.display(),
        "toolCallId": message.tool_call_id,
    })
}

/// Runs the print loop for `--mode json`. Writes one JSON line per event.
pub async fn run_json(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    header: Option<Value>,
) -> Result<()> {
    let mut stdout = std::io::stdout();
    if let Some(header) = header {
        writeln!(stdout, "{header}")?;
    }
    while let Some(event) = rx.recv().await {
        if let Some(value) = event_json(&event) {
            writeln!(stdout, "{value}")?;
            stdout.flush()?;
        }
        if matches!(event, AgentEvent::Finished(_)) {
            break;
        }
    }
    Ok(())
}

/// RPC mode: reads LF-delimited JSONL requests from stdin and writes JSONL
/// events to stdout. Each request is `{"type":"prompt","message":"..."}`.
pub async fn run_rpc(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    prompt_tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<()> {
    // Reads LF-delimited JSONL requests until stdin closes or a `quit`/`abort`
    // request arrives. Dropping `prompt_tx` on EOF lets the driver finish and
    // close the event channel, which ends the loop below.
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            match value.get("type").and_then(Value::as_str) {
                Some("prompt") => {
                    if let Some(message) = value.get("message").and_then(Value::as_str) {
                        if prompt_tx.send(message.to_string()).is_err() {
                            break;
                        }
                    }
                }
                Some("quit") | Some("abort") => break,
                _ => {}
            }
        }
    });

    let mut stdout = std::io::stdout();
    while let Some(event) = rx.recv().await {
        if let Some(value) = event_json(&event) {
            writeln!(stdout, "{value}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_filter_defaults_and_lists() {
        let all = ToolFilter::default();
        assert!(all.permits("read"));
        assert!(all.permits("bash"));

        let allow = ToolFilter::new(Some("read,bash".into()), None);
        assert!(allow.permits("read"));
        assert!(allow.permits("read_file"));
        assert!(!allow.permits("write"));

        let exclude = ToolFilter::new(None, Some("bash".into()));
        assert!(!exclude.permits("bash"));
        assert!(exclude.permits("read"));

        let both = ToolFilter::new(Some("read,write".into()), Some("write".into()));
        assert!(both.permits("read"));
        assert!(!both.permits("write_file"));
        assert!(!both.permits("bash"));
    }

    #[test]
    fn event_json_shapes() {
        let text = event_json(&AgentEvent::Text("hi".into())).unwrap();
        assert_eq!(text["type"], "message_update");
        assert_eq!(text["assistantMessageEvent"]["delta"], "hi");

        let call = event_json(&AgentEvent::ToolCall {
            name: "read".into(),
            args: "{}".into(),
        })
        .unwrap();
        assert_eq!(call["type"], "tool_call");
        assert_eq!(call["toolName"], "read");

        let result = event_json(&AgentEvent::ToolResult {
            name: "bash".into(),
            args: "{}".into(),
            output: "error: nope".into(),
            diff: None,
        })
        .unwrap();
        assert_eq!(result["isError"], true);
    }

    #[test]
    fn expands_at_file_arguments() {
        let dir = std::env::temp_dir().join(format!("oxide_cli_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("prompt.md"), "review this").unwrap();

        let args = expand_file_args(&dir, &["@prompt.md".into(), "please".into()]).unwrap();
        assert!(args.text.contains("review this"));
        assert!(args.text.contains("please"));
        assert!(args.attachments.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }
}
