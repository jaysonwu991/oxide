//! Pi-compatible non-interactive output modes and CLI argument helpers.
//!
//! `--mode json` emits every agent event as a JSON line and `--mode rpc`
//! additionally accepts JSONL input on stdin. Both use LF-delimited framing,
//! matching Pi's event-stream contract.

use crate::agent::AgentEvent;
use crate::approval::ApprovalBroker;
use crate::session::SessionLog;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// A request a front-end sends over the `--mode rpc` input channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcRequest {
    /// Start an agent turn. `images` are attachment paths for the message.
    Prompt { text: String, images: Vec<PathBuf> },
    /// Answer a pending approval request. `decision` is `deny`, `once` or
    /// `always`; a `deny` may carry a message for the agent.
    Approval {
        id: u64,
        decision: String,
        message: Option<String>,
    },
    /// End the session.
    Quit,
}

impl RpcRequest {
    /// Parses one JSONL request. Unknown shapes and unknown types return
    /// `None`, so a newer front-end can send a request this build ignores.
    pub fn parse(value: &Value) -> Option<Self> {
        match value.get("type").and_then(Value::as_str)? {
            "prompt" => {
                let text = value
                    .get("message")
                    .or_else(|| value.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let images = value
                    .get("images")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(PathBuf::from)
                            .collect()
                    })
                    .unwrap_or_default();
                Some(RpcRequest::Prompt { text, images })
            }
            "approval" => Some(RpcRequest::Approval {
                id: value.get("id").and_then(Value::as_u64)?,
                decision: value.get("decision").and_then(Value::as_str)?.to_string(),
                message: value
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }),
            "quit" | "abort" => Some(RpcRequest::Quit),
            _ => None,
        }
    }
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
        // A tool is waiting for the user's decision. The consumer answers with
        // an `approval` request on the RPC input channel (see [`RpcRequest`]).
        AgentEvent::ApprovalRequest { id, tool, detail } => json!({
            "type": "approval_request",
            "id": id,
            "toolName": tool,
            "detail": detail,
        }),
        AgentEvent::Thought { .. } => json!({ "type": "thinking" }),
        // The model step finished streaming. A consumer that renders the live
        // transcript uses this as the step boundary: text and reasoning after
        // it belong to a new step, so a retry can discard only its own attempt.
        AgentEvent::ThoughtDone { millis } => json!({ "type": "thinking_done", "millis": millis }),
        AgentEvent::ThinkingDelta(delta) => json!({
            "type": "message_update",
            "assistantMessageEvent": { "type": "thinking_delta", "delta": delta }
        }),
        AgentEvent::Retrying {
            attempt,
            max,
            delay_ms,
        } => json!({
            "type": "auto_retry_start",
            "attempt": attempt,
            "maxAttempts": max,
            "delayMs": delay_ms,
        }),
        AgentEvent::Branch { .. } => return None,
        // Nested `task` activity is progress for the interactive view; the
        // task's own result already carries everything a script needs.
        AgentEvent::SubagentActivity { .. } => return None,
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
        AgentEvent::Usage {
            input,
            output,
            cache_read,
            cache_write,
            cost,
        } => json!({
            "type": "usage",
            "usage": {
                "input": input,
                "output": output,
                "cacheRead": cache_read,
                "cacheWrite": cache_write,
                "cost": cost,
            }
        }),
        AgentEvent::Compaction {
            summary,
            summarized,
            tokens_before,
            read_files,
            modified_files,
        } => json!({
            "type": "compaction",
            "summary": summary,
            "summarized": summarized,
            "tokensBefore": tokens_before,
            "readFiles": read_files,
            "modifiedFiles": modified_files,
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
/// events to stdout. Each request is `{"type":"prompt","message":"..."}`; an
/// `{"type":"approval","id":1,"decision":"once"}` answers a pending tool
/// approval, and `quit`/`abort` ends the session.
pub async fn run_rpc(
    mut events: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    mut control: tokio::sync::mpsc::UnboundedReceiver<Value>,
    prompts: tokio::sync::mpsc::UnboundedSender<RpcRequest>,
    approvals: Option<Arc<ApprovalBroker>>,
) -> Result<()> {
    // Reads LF-delimited JSONL requests until stdin closes or a `quit`/`abort`
    // request arrives. Dropping `prompts` on EOF lets the driver finish and
    // close the event channel, which ends the loop below. Approvals are
    // answered straight from this thread, because the driver is busy streaming
    // the very turn that is waiting for the answer.
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
            match RpcRequest::parse(&value) {
                Some(request @ RpcRequest::Prompt { .. }) => {
                    if prompts.send(request).is_err() {
                        break;
                    }
                }
                Some(RpcRequest::Approval {
                    id,
                    decision,
                    message,
                }) => {
                    if let Some(broker) = &approvals {
                        broker.resolve(id, &decision, message.as_deref());
                    }
                }
                Some(RpcRequest::Quit) => break,
                None => {}
            }
        }
    });

    let mut stdout = std::io::stdout();
    let mut control_open = true;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(event) => {
                    if let Some(value) = event_json(&event) {
                        writeln!(stdout, "{value}")?;
                        stdout.flush()?;
                    }
                }
                None => break,
            },
            // Host messages interleaved with the event stream: the session
            // header, and anything else the driver needs to say.
            message = control.recv(), if control_open => match message {
                Some(value) => {
                    writeln!(stdout, "{value}")?;
                    stdout.flush()?;
                }
                None => control_open = false,
            },
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
    fn rpc_requests_parse() {
        let prompt = RpcRequest::parse(&json!({
            "type": "prompt",
            "message": "fix the build",
            "images": ["shot.png", 7, "diagram.pdf"],
        }))
        .unwrap();
        assert_eq!(
            prompt,
            RpcRequest::Prompt {
                text: "fix the build".into(),
                images: vec![PathBuf::from("shot.png"), PathBuf::from("diagram.pdf")],
            }
        );

        // `text` is accepted as an alias, with no images.
        assert_eq!(
            RpcRequest::parse(&json!({"type": "prompt", "text": "hi"})).unwrap(),
            RpcRequest::Prompt {
                text: "hi".into(),
                images: Vec::new()
            }
        );

        assert_eq!(
            RpcRequest::parse(&json!({"type": "approval", "id": 3, "decision": "always"})).unwrap(),
            RpcRequest::Approval {
                id: 3,
                decision: "always".into(),
                message: None
            }
        );
        assert_eq!(
            RpcRequest::parse(&json!({
                "type": "approval",
                "id": 4,
                "decision": "deny",
                "message": "use tabs"
            }))
            .unwrap(),
            RpcRequest::Approval {
                id: 4,
                decision: "deny".into(),
                message: Some("use tabs".into())
            }
        );

        assert_eq!(
            RpcRequest::parse(&json!({"type": "quit"})),
            Some(RpcRequest::Quit)
        );
        assert_eq!(
            RpcRequest::parse(&json!({"type": "abort"})),
            Some(RpcRequest::Quit)
        );
        // An approval without an id, and an unknown type, are ignored.
        assert_eq!(
            RpcRequest::parse(&json!({"type": "approval", "decision": "once"})),
            None
        );
        assert_eq!(RpcRequest::parse(&json!({"type": "telepathy"})), None);
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

        let usage = event_json(&AgentEvent::Usage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
            cost: 0.5,
        })
        .unwrap();
        assert_eq!(usage["usage"]["cacheRead"], 3);
        assert_eq!(usage["usage"]["cost"], 0.5);

        let result = event_json(&AgentEvent::ToolResult {
            name: "bash".into(),
            args: "{}".into(),
            output: "error: nope".into(),
            diff: None,
            millis: 0,
        })
        .unwrap();
        assert_eq!(result["isError"], true);

        let done = event_json(&AgentEvent::ThoughtDone { millis: 12 }).unwrap();
        assert_eq!(done["type"], "thinking_done");
        assert_eq!(done["millis"], 12);

        let thinking = event_json(&AgentEvent::ThinkingDelta("weighing".into())).unwrap();
        assert_eq!(thinking["assistantMessageEvent"]["type"], "thinking_delta");
        assert_eq!(thinking["assistantMessageEvent"]["delta"], "weighing");

        let retry = event_json(&AgentEvent::Retrying {
            attempt: 1,
            max: 3,
            delay_ms: 500,
        })
        .unwrap();
        assert_eq!(retry["type"], "auto_retry_start");
        assert_eq!(retry["maxAttempts"], 3);

        let approval = event_json(&AgentEvent::ApprovalRequest {
            id: 7,
            tool: "bash".into(),
            detail: "rm -rf /".into(),
        })
        .unwrap();
        assert_eq!(approval["type"], "approval_request");
        assert_eq!(approval["id"], 7);
        assert_eq!(approval["toolName"], "bash");
        assert_eq!(approval["detail"], "rm -rf /");
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
