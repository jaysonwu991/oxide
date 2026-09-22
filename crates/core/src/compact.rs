//! Pi-style context compaction.
//!
//! When a conversation approaches the model's context window, older turns are
//! replaced by a structured summary while the most recent tokens stay verbatim.
//! Compaction records are appended to the session log and replayed to rebuild
//! the outgoing view, so a resumed session sends the same compacted context.
//!
//! The algorithm mirrors Pi's `compaction` package: walk backwards from the
//! newest message until `keepRecentTokens` is reached, summarize everything
//! before that cut (including the previous summary as iterative context), track
//! read/modified files cumulatively, and never cut in the middle of a tool
//! call/result pair.

use crate::config::Config;
use crate::llm::{LlmClient, Message, Retry, StreamHooks, Usage};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_RESERVE_TOKENS: u64 = 16_384;
pub const DEFAULT_KEEP_RECENT_TOKENS: u64 = 20_000;

/// Tool output longer than this is truncated when serializing a conversation
/// for the summarizer, keeping the summarization request bounded.
const SERIALIZED_TOOL_LIMIT: usize = 2_000;

/// Compaction settings, loaded from `settings.json` and overridable per model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompactionConfig {
    pub enabled: bool,
    /// Tokens reserved for the model's response; compaction triggers once the
    /// context exceeds `contextWindow - reserveTokens`.
    pub reserve_tokens: u64,
    /// Recent tokens kept verbatim when compacting.
    pub keep_recent_tokens: u64,
    /// Per-model overrides keyed by `provider/model`.
    pub model_overrides: BTreeMap<String, ModelCompaction>,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: DEFAULT_RESERVE_TOKENS,
            keep_recent_tokens: DEFAULT_KEEP_RECENT_TOKENS,
            model_overrides: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelCompaction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_recent_tokens: Option<u64>,
}

/// Effective budgets for the active model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub enabled: bool,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
}

impl CompactionConfig {
    /// Resolves the ordinary settings against a `provider/model` override.
    pub fn resolve(&self, provider: &str, model: &str) -> Budget {
        let overrides = self.model_overrides.get(&format!("{provider}/{model}"));
        Budget {
            enabled: self.enabled,
            reserve_tokens: overrides
                .and_then(|entry| entry.reserve_tokens)
                .unwrap_or(self.reserve_tokens),
            keep_recent_tokens: overrides
                .and_then(|entry| entry.keep_recent_tokens)
                .unwrap_or(self.keep_recent_tokens),
        }
    }
}

/// Token usage that produced a summary, stored so session totals include it.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct UsageRecord {
    pub input: u64,
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub cost: f64,
}

impl From<Usage> for UsageRecord {
    fn from(usage: Usage) -> Self {
        Self {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            cost: usage.cost,
        }
    }
}

/// Cumulative file operations carried across repeated compactions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionDetails {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified_files: Vec<String>,
}

/// One persisted compaction: a summary plus the raw-history boundary it keeps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compaction {
    pub summary: String,
    /// Raw-history index of the first message kept after the summary.
    pub first_kept: usize,
    /// How many messages this compaction replaced with the summary.
    #[serde(default)]
    pub summarized: usize,
    pub tokens_before: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageRecord>,
    #[serde(default)]
    pub details: CompactionDetails,
}

/// A rough token estimate (about four characters per token).
pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages.iter().map(estimate_message_tokens_usize).sum()
}

fn estimate_message_tokens_usize(message: &Message) -> usize {
    let content = message.display().map(|text| text.len()).unwrap_or(0);
    let calls = message
        .tool_calls
        .as_ref()
        .map(|calls| {
            calls
                .iter()
                .map(|call| {
                    call.function.name.len() + call.function.arguments.len() + call.id.len()
                })
                .sum::<usize>()
        })
        .unwrap_or(0);
    (content + calls) / 4 + 4
}

/// Whether the context has crossed the auto-compaction threshold.
pub fn needs_compaction(context_tokens: u64, context_window: u64, budget: Budget) -> bool {
    budget.enabled && context_tokens > context_window.saturating_sub(budget.reserve_tokens)
}

/// A prepared summarization: which messages to summarize and where the kept
/// tail starts.
#[derive(Debug, Clone)]
pub struct Preparation {
    pub messages_to_summarize: Vec<Message>,
    pub first_kept: usize,
    pub previous_summary: Option<String>,
    pub details: CompactionDetails,
    pub tokens_before: u64,
    pub summarized: usize,
}

/// Chooses the summarization cut point for `messages`.
///
/// Walks backwards from the newest message until `keep_recent_tokens` is
/// reached; everything before that point (bounded by the previous compaction's
/// kept boundary) is summarized. Returns `None` when there is nothing new to
/// compact.
pub fn prepare(
    messages: &[Message],
    compactions: &[Compaction],
    budget: Budget,
    tokens_before: u64,
) -> Option<Preparation> {
    let previous = compactions.last();
    let start = previous
        .map(|compaction| compaction.first_kept)
        .unwrap_or(0)
        .min(messages.len());

    let mut cut = messages.len();
    let mut kept = 0u64;
    while cut > start {
        kept += estimate_message_tokens(&messages[cut - 1]);
        cut -= 1;
        if kept >= budget.keep_recent_tokens {
            break;
        }
    }
    cut = normalize_cut(messages, cut);
    if cut <= start || cut >= messages.len() {
        return None;
    }

    let messages_to_summarize = messages[start..cut].to_vec();
    let mut details = previous
        .map(|compaction| compaction.details.clone())
        .unwrap_or_default();
    let (read, modified) = extract_file_ops(&messages_to_summarize);
    merge_files(&mut details.read_files, read);
    merge_files(&mut details.modified_files, modified);

    Some(Preparation {
        first_kept: cut,
        previous_summary: previous.map(|compaction| compaction.summary.clone()),
        details,
        tokens_before,
        summarized: messages_to_summarize.len(),
        messages_to_summarize,
    })
}

/// A cut point must not fall on a tool result, which has to stay next to the
/// assistant call that produced it.
fn normalize_cut(messages: &[Message], mut cut: usize) -> usize {
    while cut < messages.len() && messages[cut].role == "tool" {
        cut += 1;
    }
    cut
}

/// Roughly four characters per token.
fn estimate_message_tokens(message: &Message) -> u64 {
    estimate_message_tokens_usize(message) as u64
}

/// Serializes a conversation for the summarizer, mirroring Pi's format so the
/// model does not treat the transcript as a conversation to continue.
pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut out = String::new();
    for message in messages {
        match message.role.as_str() {
            "user" => {
                if let Some(text) = message.display() {
                    push_block(&mut out, "[User]", &text);
                }
            }
            "assistant" => {
                if let Some(text) = message.display() {
                    if !text.trim().is_empty() {
                        push_block(&mut out, "[Assistant]", &text);
                    }
                }
                if let Some(calls) = &message.tool_calls {
                    let calls = calls
                        .iter()
                        .map(|call| {
                            format!(
                                "{}({})",
                                call.function.name,
                                compact_arguments(&call.function.arguments)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    if !calls.is_empty() {
                        push_block(&mut out, "[Assistant tool calls]", &calls);
                    }
                }
            }
            "tool" => {
                let text = message.display().unwrap_or_default();
                push_block(&mut out, "[Tool result]", &truncate_tool(&text));
            }
            _ => {}
        }
    }
    out
}

fn push_block(out: &mut String, label: &str, text: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(label);
    out.push_str(": ");
    out.push_str(text.trim());
    out.push('\n');
}

fn compact_arguments(arguments: &str) -> String {
    match serde_json::from_str::<Value>(arguments) {
        Ok(value) => {
            let text = value.to_string();
            truncate(&text, 400)
        }
        Err(_) => truncate(arguments, 400),
    }
}

fn truncate_tool(text: &str) -> String {
    if text.chars().count() <= SERIALIZED_TOOL_LIMIT {
        return text.to_string();
    }
    let kept: String = text.chars().take(SERIALIZED_TOOL_LIMIT).collect();
    let omitted = text.chars().count() - SERIALIZED_TOOL_LIMIT;
    format!("{kept}… [{omitted} characters truncated]")
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}…")
}

/// File operations from tool calls, split into reads and modifications.
pub fn extract_file_ops(messages: &[Message]) -> (Vec<String>, Vec<String>) {
    let mut read = Vec::new();
    let mut modified = Vec::new();
    for message in messages {
        let Some(calls) = &message.tool_calls else {
            continue;
        };
        for call in calls {
            let Ok(args) = serde_json::from_str::<Value>(&call.function.arguments) else {
                continue;
            };
            let Some(path) = args.get("path").and_then(Value::as_str) else {
                continue;
            };
            match crate::tools::canonical_tool_name(&call.function.name) {
                "read_file" => read.push(path.to_string()),
                "write_file" | "edit" | "patch" => modified.push(path.to_string()),
                _ => {}
            }
        }
    }
    (read, modified)
}

fn merge_files(target: &mut Vec<String>, incoming: Vec<String>) {
    for path in incoming {
        if !target.contains(&path) {
            target.push(path);
        }
    }
}

const SUMMARY_SYSTEM_PROMPT: &str = "\
You compact conversation history for another AI coding agent. Rewrite the \
conversation below as a structured handoff that preserves everything needed to \
continue the work. Do not continue the conversation or answer any question in \
it; only produce the summary. Use exactly this Markdown structure:

## Goal
[What the user is trying to accomplish]

## Constraints & Preferences
- [Requirements mentioned by the user]

## Progress
### Done
- [x] [Completed tasks]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues, if any]

## Key Decisions
- **[Decision]**: [Rationale]

## Next Steps
1. [What should happen next]

## Critical Context
- [Data needed to continue]

Be concise but complete. Preserve file paths, commands, code changes and \
unresolved tasks.";

/// Generates a compaction summary for the prepared span. `instructions`
/// optionally focuses the summary (from `/compact <instructions>`).
async fn generate_summary(
    config: &Config,
    preparation: &Preparation,
    instructions: Option<&str>,
) -> Result<(String, Usage)> {
    let transcript = serialize_conversation(&preparation.messages_to_summarize);
    let mut user = String::new();
    if let Some(previous) = &preparation.previous_summary {
        user.push_str("Summary of the earlier conversation:\n");
        user.push_str(previous);
        user.push_str("\n\n");
    }
    if let Some(instructions) = instructions.map(str::trim).filter(|text| !text.is_empty()) {
        user.push_str("Additional focus for this summary:\n");
        user.push_str(instructions);
        user.push_str("\n\n");
    }
    user.push_str("Conversation to summarize:\n\n");
    user.push_str(&transcript);
    if !preparation.details.read_files.is_empty() {
        user.push_str("\nFiles read:\n");
        for path in &preparation.details.read_files {
            user.push_str(&format!("- {path}\n"));
        }
    }
    if !preparation.details.modified_files.is_empty() {
        user.push_str("\nFiles modified:\n");
        for path in &preparation.details.modified_files {
            user.push_str(&format!("- {path}\n"));
        }
    }

    let messages = vec![Message::system(SUMMARY_SYSTEM_PROMPT), Message::user(user)];
    let client = LlmClient::new(config.clone());
    let mut summary = String::new();
    let turn = {
        let mut ignore_thinking = |_: String| {};
        let mut ignore_retry = |_: Retry| {};
        let mut hooks = StreamHooks {
            text: &mut |delta: String| summary.push_str(&delta),
            thinking: &mut ignore_thinking,
            retry: &mut ignore_retry,
        };
        client
            .stream_chat(&messages, &[], &mut hooks)
            .await
            .context("summarizing conversation")?
    };
    Ok((summary.trim().to_string(), turn.usage))
}

/// Produces a compaction record when the conversation has something to
/// compact, otherwise `None`.
pub async fn generate(
    config: &Config,
    messages: &[Message],
    compactions: &[Compaction],
    budget: Budget,
    tokens_before: u64,
    instructions: Option<&str>,
) -> Result<Option<Compaction>> {
    let Some(preparation) = prepare(messages, compactions, budget, tokens_before) else {
        return Ok(None);
    };
    let (summary, usage) = generate_summary(config, &preparation, instructions).await?;
    if summary.is_empty() {
        anyhow::bail!("summarizer returned an empty summary");
    }
    Ok(Some(Compaction {
        summary,
        first_kept: preparation.first_kept,
        summarized: preparation.summarized,
        tokens_before: preparation.tokens_before,
        usage: Some(usage.into()),
        details: preparation.details,
    }))
}

/// Summarizes the messages of an abandoned branch, mirroring Pi's branch
/// summarization so context from the path being left is preserved. Uses the
/// same structured format as compaction.
pub async fn summarize_branch(
    config: &Config,
    messages: &[Message],
    previous_summary: Option<&str>,
) -> Result<(String, Usage)> {
    let (read, modified) = extract_file_ops(messages);
    let mut details = CompactionDetails::default();
    merge_files(&mut details.read_files, read);
    merge_files(&mut details.modified_files, modified);
    let preparation = Preparation {
        messages_to_summarize: messages.to_vec(),
        first_kept: messages.len(),
        previous_summary: previous_summary.map(str::to_string),
        details,
        tokens_before: estimate_tokens(messages) as u64,
        summarized: messages.len(),
    };
    generate_summary(config, &preparation, None).await
}

/// Compacts a message list into `[summary, ...kept]` for callers that rewrite
/// history rather than keeping compaction records (`oxide sessions compact`
/// and the manual `/compact` command).
pub async fn compact_messages(
    config: &Config,
    messages: Vec<Message>,
    instructions: Option<&str>,
) -> Result<Vec<Message>> {
    let budget = config.compaction.resolve(&config.provider, &config.model);
    if messages.len() <= 1 {
        return Ok(messages);
    }
    let tokens_before = estimate_tokens(&messages) as u64;
    let Some(preparation) = prepare(&messages, &[], budget, tokens_before) else {
        return Ok(messages);
    };
    let (summary, _) = generate_summary(config, &preparation, instructions).await?;
    let mut compacted = Vec::with_capacity(expected_view_len(&messages, preparation.first_kept));
    compacted.push(Message::user(format!("[conversation summary]\n{summary}")));
    compacted.extend_from_slice(&messages[preparation.first_kept..]);
    Ok(compacted)
}

fn expected_view_len(messages: &[Message], first_kept: usize) -> usize {
    messages.len() - first_kept.min(messages.len()) + 1
}

/// Loads compaction settings from the global `settings.json` and the project
/// `.oxide/settings.json` (project wins), then applies `OXIDE_*` overrides.
pub fn load_config(cwd: &Path) -> CompactionConfig {
    let mut value = serde_json::to_value(CompactionConfig::default()).unwrap_or(Value::Null);
    for path in config_paths(cwd) {
        if let Some(overlay) = read_compaction(&path) {
            merge(&mut value, &overlay);
        }
    }
    let mut config: CompactionConfig = serde_json::from_value(value).unwrap_or_default();
    apply_env(&mut config);
    config
}

fn config_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = dirs::config_dir() {
        paths.push(dir.join("oxide").join("settings.json"));
    }
    if let Some(root) = crate::ecosystem::project_root(cwd) {
        paths.push(root.join(".oxide").join("settings.json"));
    }
    paths
}

fn read_compaction(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value.get("compaction").cloned()
}

fn merge(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge(base.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (base, overlay) => *base = overlay.clone(),
    }
}

fn apply_env(config: &mut CompactionConfig) {
    if let Some(value) = env_u64("OXIDE_COMPACTION_RESERVE_TOKENS") {
        config.reserve_tokens = value;
    }
    if let Some(value) = env_u64("OXIDE_COMPACTION_KEEP_RECENT_TOKENS") {
        config.keep_recent_tokens = value;
    }
    if let Ok(value) = std::env::var("OXIDE_COMPACTION_ENABLED") {
        if let Ok(enabled) = value.trim().parse::<bool>() {
            config.enabled = enabled;
        }
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolCall;

    fn tool_call(id: &str, name: &str, arguments: &str) -> Message {
        Message::assistant(
            "",
            vec![ToolCall {
                id: id.to_string(),
                kind: "function".to_string(),
                function: crate::llm::FunctionCall {
                    name: name.to_string(),
                    arguments: arguments.to_string(),
                },
            }],
        )
    }

    fn budget() -> Budget {
        Budget {
            enabled: true,
            reserve_tokens: 100,
            keep_recent_tokens: 8,
        }
    }

    #[test]
    fn needs_compaction_only_above_threshold() {
        let budget = Budget {
            enabled: true,
            reserve_tokens: 100,
            keep_recent_tokens: 20,
        };
        assert!(!needs_compaction(900, 1000, budget));
        assert!(needs_compaction(901, 1000, budget));
        assert!(!needs_compaction(
            1000,
            1000,
            Budget {
                enabled: false,
                ..budget
            }
        ));
    }

    #[test]
    fn prepare_keeps_recent_tokens_and_summarizes_older() {
        let messages: Vec<Message> = (0..20)
            .map(|index| Message::user(format!("message number {index} with some text")))
            .collect();
        let preparation = prepare(&messages, &[], budget(), 12).unwrap();
        assert!(preparation.first_kept > 0);
        assert!(preparation.first_kept < messages.len());
        assert_eq!(
            preparation.messages_to_summarize.len(),
            preparation.first_kept
        );
    }

    #[test]
    fn prepare_never_cuts_between_tool_call_and_result() {
        let messages = vec![
            Message::user("read the file"),
            tool_call("a", "read_file", r#"{"path":"src/a.rs"}"#),
            Message::tool("a", "file contents"),
            Message::user("thanks"),
        ];
        let preparation = prepare(
            &messages,
            &[],
            Budget {
                enabled: true,
                reserve_tokens: 10,
                keep_recent_tokens: 1,
            },
            5,
        )
        .unwrap();
        assert_ne!(messages[preparation.first_kept].role, "tool");
        let (read, modified) = extract_file_ops(&preparation.messages_to_summarize);
        assert_eq!(read, vec!["src/a.rs"]);
        assert!(modified.is_empty());
    }

    #[test]
    fn repeated_compaction_starts_at_previous_boundary() {
        let messages: Vec<Message> = (0..20)
            .map(|index| Message::user(format!("message {index} padding padding")))
            .collect();
        let first = Compaction {
            summary: "first".into(),
            first_kept: 10,
            summarized: 10,
            tokens_before: 100,
            usage: None,
            details: CompactionDetails {
                read_files: vec!["a".into()],
                modified_files: vec![],
            },
        };
        let preparation = prepare(
            &messages,
            std::slice::from_ref(&first),
            Budget {
                enabled: true,
                reserve_tokens: 10,
                keep_recent_tokens: 1,
            },
            50,
        )
        .unwrap();
        assert_eq!(preparation.previous_summary.as_deref(), Some("first"));
        assert!(preparation.first_kept > first.first_kept);
        assert!(preparation.details.read_files.contains(&"a".to_string()));
    }

    #[test]
    fn serialize_conversation_matches_pi_shape() {
        let messages = vec![
            Message::user("do it"),
            tool_call("a", "bash", r#"{"command":"ls"}"#),
            Message::tool("a", "file.rs\n".repeat(800).as_str()),
        ];
        let text = serialize_conversation(&messages);
        assert!(text.contains("[User]: do it"));
        assert!(text.contains("[Assistant tool calls]: bash("));
        assert!(text.contains("[Tool result]:"));
        assert!(text.contains("characters truncated"));
    }

    #[test]
    fn resolve_applies_model_override() {
        let mut config = CompactionConfig::default();
        config.model_overrides.insert(
            "openai/gpt-4o".into(),
            ModelCompaction {
                reserve_tokens: Some(400_000),
                keep_recent_tokens: None,
            },
        );
        let budget = config.resolve("openai", "gpt-4o");
        assert_eq!(budget.reserve_tokens, 400_000);
        assert_eq!(budget.keep_recent_tokens, DEFAULT_KEEP_RECENT_TOKENS);
        let other = config.resolve("openai", "other");
        assert_eq!(other.reserve_tokens, DEFAULT_RESERVE_TOKENS);
    }

    #[test]
    fn merge_overlays_nested_objects() {
        let mut base =
            serde_json::json!({"reserveTokens": 1, "modelOverrides": {"a": {"reserveTokens": 2}}});
        merge(
            &mut base,
            &serde_json::json!({"keepRecentTokens": 3, "modelOverrides": {"b": {}}}),
        );
        assert_eq!(base["keepRecentTokens"], 3);
        assert_eq!(base["modelOverrides"]["a"]["reserveTokens"], 2);
        assert!(base["modelOverrides"]["b"].is_object());
    }
}
