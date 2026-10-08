//! Non-interactive saved-session management: list, delete, compact, and merge.

use crate::compact;
use crate::config::Config;
use crate::llm::{ContentPart, Message, MessageContent};
use crate::session::{project_dir, SessionLog, SessionSummary, UsageTotals};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn list(cwd: &Path, all: bool, older_than_days: Option<u64>) -> Result<()> {
    let mut sessions = if all {
        SessionLog::list_all()?
    } else {
        SessionLog::list(cwd)?
    };
    if let Some(days) = older_than_days {
        let cutoff = now_secs().saturating_sub(days.saturating_mul(86_400));
        sessions.retain(|summary| summary.modified_at < cutoff);
    }
    if sessions.is_empty() {
        println!("no sessions{}", if all { "" } else { " for this project" });
        return Ok(());
    }

    let now = now_secs();
    for summary in &sessions {
        let label = summary
            .name
            .clone()
            .unwrap_or_else(|| summary.preview.clone());
        if all {
            println!(
                "{}  {:<10}  {:>4} msg  {}  {}",
                summary.id,
                relative_age(now, summary.modified_at),
                summary.message_count,
                summary.cwd,
                label
            );
        } else {
            println!(
                "{}  {:<10}  {:>4} msg  {}",
                summary.id,
                relative_age(now, summary.modified_at),
                summary.message_count,
                label
            );
        }
    }
    Ok(())
}

pub fn delete(
    cwd: &Path,
    id: Option<String>,
    all: bool,
    older_than_days: Option<u64>,
    force: bool,
) -> Result<()> {
    let targets = delete_targets(&project_dir(cwd), id, all, older_than_days)?;
    if targets.is_empty() {
        println!("no sessions to delete");
        return Ok(());
    }

    if !force {
        println!("delete {} session(s)?", targets.len());
        let now = now_secs();
        for summary in &targets {
            let label = summary
                .name
                .clone()
                .unwrap_or_else(|| summary.preview.clone());
            println!(
                "  {}  {}  {}",
                summary.id,
                relative_age(now, summary.modified_at),
                label
            );
        }
        print!("[y/N] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("aborted");
            return Ok(());
        }
    }

    remove_targets(&targets)
}

/// Removes the files a delete settled on. Each is removed by the path it was
/// resolved to: resolving the id again here is free to land on a different file
/// — a copy of the thread, or a file that was renamed — which is how a delete
/// could take a session the user never named.
fn remove_targets(targets: &[SessionSummary]) -> Result<()> {
    for summary in targets {
        SessionLog::delete_path(&summary.path)?;
        println!("deleted {}", summary.id);
    }
    Ok(())
}

/// Prints one saved session: the conversation still in its context (the leaf
/// path with the latest compaction applied), oldest first. `tail` keeps only the
/// newest `n` messages, and `json` prints the same thing as an object a
/// front-end draws — the shape `oxide mcp list --json` has for the server list.
///
/// This is how a client shows a thread it is about to resume: the CLI owns the
/// session layout, so nothing else has to agree with the core about how a
/// project's session directory or its entry tree is shaped.
pub fn show(cwd: &Path, id: &str, tail: Option<usize>, json: bool) -> Result<()> {
    let log = SessionLog::open_ref(cwd, id)?;
    let messages = log.messages()?;
    let summary = log.summary()?;
    let totals = log.usage_totals();
    print!(
        "{}",
        render_show(
            &messages,
            &summary,
            totals,
            log.context_tokens(),
            tail,
            json
        )
    );
    Ok(())
}

/// The whole of `show`'s output, so the two forms are testable without a
/// session on disk or a configured provider.
fn render_show(
    messages: &[Message],
    summary: &SessionSummary,
    totals: UsageTotals,
    context_tokens: u64,
    tail: Option<usize>,
    json: bool,
) -> String {
    let total = messages.len();
    let start = tail.map_or(0, |n| total.saturating_sub(n));
    let kept = &messages[start..];
    let mut out = String::new();
    if json {
        let usage = json!({
            "input": totals.input,
            "output": totals.output,
            "cacheRead": totals.cache_read,
            "cacheWrite": totals.cache_write,
            "cost": totals.cost,
            "cacheHitRate": totals.cache_hit_rate,
            "contextTokens": context_tokens,
            "messageCount": total,
        });
        let view = json!({
            "id": summary.id,
            "name": summary.name,
            "cwd": summary.cwd,
            "path": summary.path.display().to_string(),
            "messageCount": total,
            "shown": kept.len(),
            "messages": kept.iter().map(message_view).collect::<Vec<_>>(),
            "usage": usage,
        });
        out.push_str(&format!("{view}\n"));
        return out;
    }

    let name = summary.name.as_deref().unwrap_or(summary.preview.as_str());
    out.push_str(&format!(
        "{}  {}  {} message{}\n",
        summary.id,
        name,
        total,
        if total == 1 { "" } else { "s" }
    ));
    out.push_str(&format!("{}\n", summary.cwd));
    if kept.len() < total {
        out.push_str(&format!(
            "showing the newest {} of {total} messages\n",
            kept.len()
        ));
    }
    for message in kept {
        let body = render_message(message);
        if body.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{}:\n{body}\n", message.role));
    }
    out
}

/// One message as JSON, with the same field names the desktop's own view of a
/// stored thread uses, so a front-end that reads one reads the other.
fn message_view(message: &Message) -> Value {
    let calls = message
        .tool_calls
        .as_ref()
        .map(|calls| {
            calls
                .iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "name": call.function.name,
                        "arguments": call.function.arguments,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "role": message.role,
        "content": message
            .content
            .as_ref()
            .map(MessageContent::display)
            .unwrap_or_default(),
        "attachments": message_attachments(message),
        "toolCalls": calls,
        "toolCallId": message.tool_call_id,
    })
}

/// Media the message carried, so a reopened thread can still preview it instead
/// of reducing it to the `[image]` marker in `content`. The desktop app's own
/// view of a stored thread carries the same two fields under the same names,
/// which is what its doc comment promises and what a front-end that reads one
/// reads the other by.
fn message_attachments(message: &Message) -> Vec<Value> {
    let Some(MessageContent::Parts(parts)) = &message.content else {
        return Vec::new();
    };
    parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::ImageUrl { image_url } => Some(json!({
                "name": "image",
                "dataUrl": image_url.url,
            })),
            ContentPart::File { file } => Some(json!({
                "name": file.filename.clone().unwrap_or_else(|| "document".into()),
                "dataUrl": file.file_data,
            })),
            ContentPart::Text { .. } => None,
        })
        .collect()
}

/// One message as text: its content, or a note naming the tools it called when
/// it carried no text (an assistant step that only dispatched tools).
fn render_message(message: &Message) -> String {
    let text = message
        .content
        .as_ref()
        .map(MessageContent::display)
        .unwrap_or_default();
    if !text.trim().is_empty() {
        return text.trim_end().to_string();
    }
    let names = message
        .tool_calls
        .as_ref()
        .map(|calls| {
            calls
                .iter()
                .map(|call| call.function.name.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if names.is_empty() {
        return String::new();
    }
    format!("({} tool call(s): {})", names.len(), names.join(", "))
}

pub async fn compact_sessions(
    cwd: &Path,
    config: &Config,
    id: Option<String>,
    all: bool,
) -> Result<()> {
    let ids = if all {
        SessionLog::list(cwd)?
            .into_iter()
            .map(|summary| summary.id)
            .collect::<Vec<_>>()
    } else if let Some(id) = id {
        vec![id]
    } else {
        anyhow::bail!("specify a session id or --all");
    };
    if ids.is_empty() {
        println!("no sessions for this project");
        return Ok(());
    }

    let mut compacted = 0;
    for id in &ids {
        match compact_one(cwd, config, id).await {
            Ok(true) => {
                println!("compacted {id}");
                compacted += 1;
            }
            Ok(false) => println!("skipped {id} (already compact)"),
            Err(err) => println!("error compacting {id}: {err:#}"),
        }
    }
    if compacted == 0 {
        println!("no sessions needed compaction");
    }
    Ok(())
}

pub async fn merge(
    cwd: &Path,
    config: Option<&Config>,
    a: &str,
    b: &str,
    summarize: bool,
) -> Result<()> {
    let log_a = SessionLog::open_ref(cwd, a)?;
    let log_b = SessionLog::open_ref(cwd, b)?;
    let a_messages = log_a.messages()?;
    let b_messages = log_b.messages()?;

    let mut merged = if summarize {
        let config = config.context("summarization requires a configured provider")?;
        compact::compact_messages(config, b_messages, None).await?
    } else {
        b_messages
    };
    merged.extend(a_messages);

    let log = SessionLog::fork(cwd, &merged)?;
    println!("merged {a} + {b} into {}", log.id());
    Ok(())
}

async fn compact_one(cwd: &Path, config: &Config, id: &str) -> Result<bool> {
    let log = SessionLog::open_id(cwd, id)?;
    let messages = log.messages()?;
    let before = messages.len();
    let compacted = compact::compact_messages(config, messages, None).await?;
    if compacted.len() == before {
        return Ok(false);
    }
    log.rewrite(&compacted)?;
    Ok(true)
}

fn delete_targets(
    dir: &Path,
    id: Option<String>,
    all: bool,
    older_than_days: Option<u64>,
) -> Result<Vec<SessionSummary>> {
    if let Some(id) = id {
        let log = SessionLog::open_id_in(dir, &id)?;
        return Ok(vec![log.summary()?]);
    }
    if !all && older_than_days.is_none() {
        anyhow::bail!("specify a session id, --all, or --older-than <days>");
    }
    let mut sessions = SessionLog::list_in(dir)?;
    if let Some(days) = older_than_days {
        let cutoff = now_secs().saturating_sub(days.saturating_mul(86_400));
        sessions.retain(|summary| summary.modified_at < cutoff);
    }
    Ok(sessions)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn relative_age(now: u64, then: u64) -> String {
    let secs = now.saturating_sub(then);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{FunctionCall, ImageUrl, ToolCall};
    use std::path::PathBuf;

    fn summary() -> SessionSummary {
        SessionSummary {
            id: "77f032d6".into(),
            name: Some("Fix these 2".into()),
            preview: "Fix these 2".into(),
            cwd: "/repo".into(),
            path: PathBuf::from("/sessions/77f032d6.jsonl"),
            modified_at: 0,
            created_at: 0,
            message_count: 4,
        }
    }

    fn conversation() -> Vec<Message> {
        let call = ToolCall {
            id: "call_1".into(),
            kind: "function".into(),
            signature: None,
            function: FunctionCall {
                name: "read".into(),
                arguments: "{\"path\":\"a.rs\"}".into(),
            },
        };
        vec![
            Message::user("hello"),
            Message::assistant("", vec![call]),
            Message::tool("call_1", "fn main() {}"),
            Message::assistant("done", Vec::new()),
        ]
    }

    fn totals() -> UsageTotals {
        UsageTotals {
            input: 100,
            output: 40,
            cache_read: 900,
            cache_write: 0,
            cost: 0.25,
            cache_hit_rate: Some(90.0),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_sessions_{tag}_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_delete_removes_the_file_the_id_resolved_to() {
        // Two files carrying one id, which is what a copied or restored session
        // file looks like: the name `deadbeef` resolves to the copy, while the
        // copy's own header still names the original. Deleting `deadbeef` has to
        // take the copy — resolving the copy's id again would find the original
        // by name and remove a thread the user never asked to delete.
        let dir = temp_dir("delete_targets");
        let cwd = temp_dir("delete_targets_proj");
        let original = SessionLog::create_in(&dir, &cwd).unwrap();
        original.append(&Message::user("keep me")).unwrap();
        let copy = dir.join("1700000000000_deadbeef.jsonl");
        std::fs::copy(original.path(), &copy).unwrap();

        let targets = delete_targets(&dir, Some("deadbeef".into()), false, None).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].path, copy, "the name the id had wins");
        assert_eq!(
            targets[0].id,
            original.id(),
            "its header still names the original"
        );

        remove_targets(&targets).unwrap();
        assert!(original.path().exists(), "the original was left alone");
        assert!(!copy.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn json_carries_the_conversation_and_what_it_cost() {
        let out = render_show(&conversation(), &summary(), totals(), 1_000, None, true);
        let view: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(view["id"], "77f032d6");
        assert_eq!(view["name"], "Fix these 2");
        assert_eq!(view["messageCount"], 4);
        assert_eq!(view["shown"], 4);
        assert_eq!(view["messages"][0]["role"], "user");
        assert_eq!(view["messages"][0]["content"], "hello");
        assert_eq!(view["messages"][1]["content"], "");
        assert_eq!(view["messages"][1]["toolCalls"][0]["name"], "read");
        assert_eq!(view["messages"][2]["role"], "tool");
        assert_eq!(view["messages"][2]["toolCallId"], "call_1");
        assert_eq!(view["usage"]["cacheRead"], 900);
        assert_eq!(view["usage"]["contextTokens"], 1_000);
        assert_eq!(view["usage"]["cacheHitRate"], 90.0);
    }

    #[test]
    fn a_stored_message_reports_the_media_it_carried() {
        // The desktop app's own view of a stored thread reports `name` and
        // `dataUrl` under the same field names, and its doc comment promises the
        // two agree; a front-end that replays a thread gets the media from here
        // or reduces a screenshot to the `[image]` marker `content` holds.
        let mut conversation = conversation();
        conversation[0] = Message::user_parts(
            "what is this?",
            vec![ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,QUJD".into(),
                    detail: None,
                },
            }],
        );
        let out = render_show(&conversation, &summary(), totals(), 0, None, true);
        let view: Value = serde_json::from_str(out.trim()).unwrap();
        assert!(view["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("[image]"));
        assert_eq!(
            view["messages"][0]["attachments"],
            json!([{ "name": "image", "dataUrl": "data:image/png;base64,QUJD" }])
        );
        // A message with no media carries an empty list rather than nothing, so
        // a reader can tell the two apart without guessing.
        assert_eq!(view["messages"][1]["attachments"], json!([]));
    }

    #[test]
    fn tail_keeps_the_newest_messages_and_says_so() {
        let out = render_show(&conversation(), &summary(), totals(), 0, Some(2), true);
        let view: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(view["messageCount"], 4);
        assert_eq!(view["shown"], 2);
        assert_eq!(view["messages"][0]["role"], "tool");
        assert_eq!(view["messages"][1]["content"], "done");

        let text = render_show(&conversation(), &summary(), totals(), 0, Some(2), false);
        assert!(
            text.contains("showing the newest 2 of 4 messages"),
            "{text}"
        );
        assert!(!text.contains("hello"), "a cut message is not printed");
        assert!(text.contains("assistant:\ndone"));
    }

    #[test]
    fn a_tail_larger_than_the_thread_keeps_everything() {
        let text = render_show(&conversation(), &summary(), totals(), 0, Some(50), false);
        assert!(!text.contains("showing the newest"), "{text}");
        assert!(text.contains("user:\nhello"));
    }

    #[test]
    fn the_text_form_names_roles_and_reports_tool_only_steps() {
        let text = render_show(&conversation(), &summary(), totals(), 0, None, false);
        assert!(
            text.starts_with("77f032d6  Fix these 2  4 messages\n/repo\n"),
            "{text}"
        );
        assert!(text.contains("assistant:\n(1 tool call(s): read)"));
        assert!(text.contains("tool:\nfn main() {}"));
    }
}
