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
use std::sync::Mutex;

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
    /// When the cut lands inside a turn (the kept tail starts with an assistant
    /// or tool message), the part of that turn before the cut. Pi summarizes it
    /// on its own and merges it into the history summary so the request that
    /// opened the turn is not lost.
    pub turn_prefix: Vec<Message>,
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

    // A turn starts at a user message. Cutting inside a turn (the kept tail
    // begins with an assistant or tool result) leaves that turn's earlier half
    // out of the kept tail, so it is summarized on its own and merged in, the
    // way Pi handles a split user-message span.
    let mut turn_prefix = Vec::new();
    let mut history_end = cut;
    if messages[cut].role != "user" {
        if let Some(turn_start) = (start..cut)
            .rev()
            .find(|&index| messages[index].role == "user")
        {
            history_end = turn_start;
            turn_prefix = messages[turn_start..cut].to_vec();
        }
    }
    let messages_to_summarize = messages[start..history_end].to_vec();
    if messages_to_summarize.is_empty() && turn_prefix.is_empty() {
        return None;
    }

    let mut details = previous
        .map(|compaction| compaction.details.clone())
        .unwrap_or_default();
    let (read, modified) = extract_file_ops(&messages_to_summarize);
    merge_files(&mut details.read_files, read);
    merge_files(&mut details.modified_files, modified);
    let (read, modified) = extract_file_ops(&turn_prefix);
    merge_files(&mut details.read_files, read);
    merge_files(&mut details.modified_files, modified);

    Some(Preparation {
        first_kept: cut,
        previous_summary: previous.map(|compaction| compaction.summary.clone()),
        details,
        tokens_before,
        summarized: messages_to_summarize.len() + turn_prefix.len(),
        messages_to_summarize,
        turn_prefix,
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
You are a context summarization assistant. Your task is to read a conversation \
between a user and an AI assistant, then produce a structured summary in the \
exact format specified. Do NOT continue the conversation or respond to any \
question in it. ONLY output the structured summary.";

/// The format block is sent *after* the conversation. Ordering the prompt this
/// way matters: with the transcript last the model tends to continue it (the
/// observed compaction summary was an echoed assistant reply), so the final
/// instruction must be the request to summarize.
const SUMMARY_INSTRUCTIONS: &str = "\
The conversation above is history to summarize, not a conversation to continue. \
Create a structured context checkpoint that another agent will use to continue \
the work. Use exactly this Markdown structure:

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

Be concise but complete. Preserve exact file paths, commands, code changes and \
unresolved tasks.";

/// Sent instead of [`SUMMARY_INSTRUCTIONS`] when an earlier summary exists, so
/// the model updates that checkpoint rather than writing a fresh one. Mirrors
/// Pi's `UPDATE_SUMMARIZATION_PROMPT`.
const UPDATE_SUMMARY_INSTRUCTIONS: &str = "\
The messages above are NEW conversation messages to incorporate into the existing \
summary provided in <previous-summary> tags. PRESERVE all existing information, \
add new progress, decisions and context, move completed items to Done, and update \
Next Steps. Use exactly this Markdown structure:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work]

### Blocked
- [Current blockers, remove resolved ones]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names and error \
messages.";

/// The prefix of a split user-message span: the request that opened the turn and
/// the progress made before the kept tail begins. Mirrors Pi's
/// `TURN_PREFIX_SUMMARIZATION_PROMPT`.
const TURN_PREFIX_SUMMARY_INSTRUCTIONS: &str = "\
The messages above are earlier context from an ongoing conversation. Later \
messages are stored separately and do not need to be reconstructed. Create a \
concise checkpoint of the user's request and the progress shown above so the \
conversation can continue with the necessary context.

## Original Request
[What did the user ask for?]

## Progress So Far
- [Key decisions and work completed in these messages]

## Context Needed to Continue
- [Information from these messages needed to understand the later work]

Only summarize information explicitly present above. Do not infer or recreate \
later messages.";

/// Caps a summarization response the way Pi does: a fraction of the reserved
/// response budget, never above the model's configured output limit.
fn summary_max_tokens(budget: Option<Budget>, config: &Config, factor: f64) -> u32 {
    let reserved = budget.map(|budget| budget.reserve_tokens).unwrap_or(0) as f64 * factor;
    let ceiling = if config.max_tokens > 0 {
        config.max_tokens as f64
    } else {
        f64::INFINITY
    };
    reserved.min(ceiling).floor().max(1.0) as u32
}

#[derive(Default)]
struct SummaryBuffer(Mutex<String>);

impl SummaryBuffer {
    fn push(&self, delta: &str) {
        self.0
            .lock()
            .expect("summary buffer poisoned")
            .push_str(delta);
    }

    fn reset(&self) {
        self.0.lock().expect("summary buffer poisoned").clear();
    }

    fn finish(self) -> String {
        self.0.into_inner().expect("summary buffer poisoned")
    }
}

/// One summarization call: the system prompt, the prompt text, and a capped
/// output budget. A one-off summary is not worth caching.
async fn complete_summary(
    config: &Config,
    prompt: String,
    max_tokens: u32,
) -> Result<(String, Usage)> {
    let mut config = config.clone();
    if max_tokens > 0 {
        config.max_tokens = max_tokens;
    }
    let messages = vec![
        Message::system(SUMMARY_SYSTEM_PROMPT),
        Message::user(prompt),
    ];
    let client = LlmClient::new(config);
    let summary = SummaryBuffer::default();
    let turn = {
        let mut ignore_thinking = |_: String| {};
        let mut append_summary = |delta: String| summary.push(&delta);
        let mut reset_summary = |_: Retry| summary.reset();
        let mut hooks = StreamHooks {
            text: &mut append_summary,
            thinking: &mut ignore_thinking,
            retry: &mut reset_summary,
        };
        client
            .stream_chat(&messages, &[], &mut hooks)
            .await
            .context("summarizing conversation")?
    };
    let summary = summary.finish();
    Ok((summary.trim().to_string(), turn.usage))
}

/// Generates the summary for one span. `instructions` optionally focuses the
/// summary (from `/compact <instructions>`).
async fn generate_summary(
    config: &Config,
    messages: &[Message],
    previous_summary: Option<&str>,
    details: &CompactionDetails,
    instructions: Option<&str>,
    max_tokens: u32,
) -> Result<(String, Usage)> {
    let user = summary_prompt(messages, previous_summary, details, instructions);
    complete_summary(config, user, max_tokens).await
}

/// Generates the checkpoint for the prefix of a split user-message span.
async fn generate_turn_prefix_summary(
    config: &Config,
    messages: &[Message],
    max_tokens: u32,
) -> Result<(String, Usage)> {
    let transcript = serialize_conversation(messages);
    let prompt = format!(
        "# Conversation\n{transcript}\n\n# Instructions\n{TURN_PREFIX_SUMMARY_INSTRUCTIONS}"
    );
    complete_summary(config, prompt, max_tokens).await
}

fn combine_usage(base: Usage, extra: Usage) -> Usage {
    Usage {
        input: base.input + extra.input,
        output: base.output + extra.output,
        cache_read: base.cache_read + extra.cache_read,
        cache_write: base.cache_write + extra.cache_write,
        reasoning: base.reasoning + extra.reasoning,
        cost: base.cost + extra.cost,
    }
}

/// Summarizes a prepared span, merging the history summary with the checkpoint
/// of a split turn's prefix when the cut landed inside one.
async fn summarize_preparation(
    config: &Config,
    preparation: &Preparation,
    budget: Budget,
    instructions: Option<&str>,
) -> Result<(String, Usage)> {
    let history_cap = summary_max_tokens(Some(budget), config, 0.8);
    if preparation.turn_prefix.is_empty() {
        return generate_summary(
            config,
            &preparation.messages_to_summarize,
            preparation.previous_summary.as_deref(),
            &preparation.details,
            instructions,
            history_cap,
        )
        .await;
    }
    let (history, history_usage) = if preparation.messages_to_summarize.is_empty() {
        (
            preparation
                .previous_summary
                .clone()
                .unwrap_or_else(|| "No prior history.".to_string()),
            None,
        )
    } else {
        let (text, usage) = generate_summary(
            config,
            &preparation.messages_to_summarize,
            preparation.previous_summary.as_deref(),
            &preparation.details,
            instructions,
            history_cap,
        )
        .await?;
        (text, Some(usage))
    };
    let prefix_cap = summary_max_tokens(Some(budget), config, 0.5);
    let (prefix, prefix_usage) =
        generate_turn_prefix_summary(config, &preparation.turn_prefix, prefix_cap).await?;
    let summary = format!("{history}\n\n---\n\n**Turn Context (split turn):**\n\n{prefix}");
    let usage = history_usage.map_or(prefix_usage, |usage| combine_usage(usage, prefix_usage));
    Ok((summary, usage))
}

/// Builds the summarizer's user message: transcript first, then the request to
/// summarize, so the final instruction is not "continue this conversation".
fn summary_prompt(
    messages: &[Message],
    previous_summary: Option<&str>,
    details: &CompactionDetails,
    instructions: Option<&str>,
) -> String {
    let transcript = serialize_conversation(messages);
    let mut user = String::new();
    user.push_str("<conversation>\n");
    user.push_str(&transcript);
    user.push_str("\n</conversation>\n\n");
    if let Some(previous) = previous_summary {
        user.push_str("<previous-summary>\n");
        user.push_str(previous);
        user.push_str("\n</previous-summary>\n\n");
    }
    if !details.read_files.is_empty() {
        user.push_str("Files read so far:\n");
        for path in &details.read_files {
            user.push_str(&format!("- {path}\n"));
        }
        user.push('\n');
    }
    if !details.modified_files.is_empty() {
        user.push_str("Files modified so far:\n");
        for path in &details.modified_files {
            user.push_str(&format!("- {path}\n"));
        }
        user.push('\n');
    }
    user.push_str(if previous_summary.is_some() {
        UPDATE_SUMMARY_INSTRUCTIONS
    } else {
        SUMMARY_INSTRUCTIONS
    });
    if let Some(instructions) = instructions.map(str::trim).filter(|text| !text.is_empty()) {
        user.push_str("\n\nAdditional focus: ");
        user.push_str(instructions);
    }
    user
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
    let (summary, usage) =
        summarize_preparation(config, &preparation, budget, instructions).await?;
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
        turn_prefix: Vec::new(),
        first_kept: messages.len(),
        previous_summary: previous_summary.map(str::to_string),
        details,
        tokens_before: estimate_tokens(messages) as u64,
        summarized: messages.len(),
    };
    let budget = config.compaction.resolve(&config.provider, &config.model);
    summarize_preparation(config, &preparation, budget, None).await
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
    let (summary, _) = summarize_preparation(config, &preparation, budget, instructions).await?;
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
    if let Some(dir) = crate::config::config_dir() {
        paths.push(dir.join("settings.json"));
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
                signature: None,
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
    fn a_summary_retry_discards_the_partial_attempt() {
        let summary = SummaryBuffer::default();
        summary.push("partial");
        summary.reset();
        summary.push("complete");
        assert_eq!(summary.finish(), "complete");
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
    fn summary_prompt_puts_the_request_after_the_conversation() {
        let details = CompactionDetails {
            read_files: vec!["/a.rs".into()],
            modified_files: vec!["/b.rs".into()],
        };
        let messages = vec![
            Message::user("build the thing"),
            Message::assistant("Done.", vec![]),
        ];
        let prompt = summary_prompt(
            &messages,
            Some("earlier summary"),
            &details,
            Some("focus on tests"),
        );

        // The transcript is wrapped and the summarize request comes last, so a
        // model cannot mistake the final assistant turn for the instruction.
        let conversation = prompt.find("<conversation>").unwrap();
        let goal = prompt.find("## Goal").unwrap();
        assert!(conversation < goal, "{prompt}");
        assert!(
            prompt.contains("<previous-summary>\nearlier summary\n</previous-summary>"),
            "{prompt}"
        );
        assert!(prompt.contains("Files read so far:\n- /a.rs"), "{prompt}");
        assert!(
            prompt.contains("Files modified so far:\n- /b.rs"),
            "{prompt}"
        );
        assert!(
            prompt
                .trim_end()
                .ends_with("Additional focus: focus on tests"),
            "{prompt}"
        );
        // A previous summary updates that checkpoint instead of writing a fresh one.
        assert!(prompt.contains("NEW conversation messages"), "{prompt}");
    }

    #[test]
    fn summary_prompt_without_previous_uses_the_initial_format() {
        let prompt = summary_prompt(
            &[Message::user("build the thing")],
            None,
            &CompactionDetails::default(),
            None,
        );
        assert!(
            prompt.contains("Create a structured context checkpoint"),
            "{prompt}"
        );
        assert!(!prompt.contains("NEW conversation messages"), "{prompt}");
    }

    #[test]
    fn prepare_splits_a_turn_when_the_cut_lands_inside_one() {
        // The newest messages exceed the budget, so the cut lands inside the
        // second user turn; its opening request is the turn prefix.
        let messages = vec![
            Message::user("first request with plenty of words to estimate"),
            Message::assistant("first answer with plenty of words to estimate", vec![]),
            Message::user("second request"),
            Message::assistant("second answer with words to estimate", vec![]),
            Message::assistant("second answer continues with words", vec![]),
        ];
        let preparation = prepare(
            &messages,
            &[],
            Budget {
                enabled: true,
                reserve_tokens: 10,
                keep_recent_tokens: 1,
            },
            100,
        )
        .unwrap();
        assert_eq!(preparation.turn_prefix[0].role, "user");
        assert_eq!(preparation.messages_to_summarize[0].role, "user");
        assert_eq!(preparation.first_kept, 4);
        let prefix_len = preparation.turn_prefix.len();
        let history_len = preparation.messages_to_summarize.len();
        assert_eq!(preparation.summarized, prefix_len + history_len);
    }

    #[test]
    fn prepare_cutting_at_a_user_message_is_not_a_split_turn() {
        let messages: Vec<Message> = (0..8)
            .map(|index| Message::user(format!("message {index} with words for the estimate")))
            .collect();
        let preparation = prepare(
            &messages,
            &[],
            Budget {
                enabled: true,
                reserve_tokens: 10,
                keep_recent_tokens: 20,
            },
            100,
        )
        .unwrap();
        assert!(preparation.turn_prefix.is_empty());
        assert_eq!(messages[preparation.first_kept].role, "user");
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
