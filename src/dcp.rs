//! Dynamic context pruning, adapted from the ideas of opencode-dynamic-context-pruning.
//!
//! The session history is never modified. Instead, before each model request we
//! build a pruned *view* of the conversation: closed spans replaced with model
//! written summaries, duplicate tool results elided, and errored tool outputs
//! purged. Compression records are persisted in the session log so a resumed
//! session reconstructs the same view.

use crate::ecosystem::project_root;
use crate::llm::{FunctionSpec, Message, ToolSpec};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const DEFAULT_PROTECTED_TOOLS: &[&str] = &[
    "task",
    "skill",
    "memory",
    "diagnostics",
    "compress",
    "write_file",
    "patch",
    "edit",
];

const DEFAULT_COMPRESS_PROTECTED_TOOLS: &[&str] = &["task", "skill", "memory", "diagnostics"];

/// The subset of dynamic-context-pruning behavior oxide implements, loaded
/// from `<project>/.oxide/dcp.json` (project) and the global config dir
/// `dcp.json`, with the project overriding the global file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DcpConfig {
    pub enabled: bool,
    pub compress: CompressConfig,
    pub strategies: Strategies,
    /// Extra tools protected from pruning.
    pub protected_tools: Vec<String>,
    /// Glob patterns for file arguments protected from pruning.
    pub protected_file_patterns: Vec<String>,
}

impl Default for DcpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            compress: CompressConfig::default(),
            strategies: Strategies::default(),
            protected_tools: Vec::new(),
            protected_file_patterns: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompressConfig {
    pub mode: String,
    pub permission: String,
    /// Above this estimated token count, strong compression nudges fire.
    pub max_context_limit: usize,
    /// At or above this estimated token count, soft nudges are considered.
    pub min_context_limit: usize,
    /// Fire soft nudges every N model iterations since the last user message.
    pub nudge_frequency: usize,
    /// Always nudge once this many iterations have passed since a user message.
    pub iteration_nudge_threshold: usize,
    /// Suppress compression nudges until this many messages have been added
    /// since the end of the last compression, so compression does not run away.
    pub cooldown_messages: usize,
    /// Never compress the most recent N messages, keeping recent file reads in
    /// context so the model does not have to re-read them.
    pub protected_recent_messages: usize,
    /// Tool outputs appended to compression summaries instead of discarded.
    pub protected_tools: Vec<String>,
    pub protect_user_messages: bool,
}

impl Default for CompressConfig {
    fn default() -> Self {
        Self {
            mode: "range".to_string(),
            permission: "allow".to_string(),
            max_context_limit: 32_000,
            min_context_limit: 16_000,
            nudge_frequency: 5,
            iteration_nudge_threshold: 15,
            cooldown_messages: 20,
            protected_recent_messages: 8,
            protected_tools: DEFAULT_COMPRESS_PROTECTED_TOOLS
                .iter()
                .map(|name| name.to_string())
                .collect(),
            protect_user_messages: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Strategies {
    pub deduplication: Deduplication,
    pub purge_errors: PurgeErrors,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Deduplication {
    pub enabled: bool,
    pub protected_tools: Vec<String>,
}

impl Default for Deduplication {
    fn default() -> Self {
        Self {
            enabled: true,
            protected_tools: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PurgeErrors {
    pub enabled: bool,
    pub turns: usize,
    pub protected_tools: Vec<String>,
}

impl Default for PurgeErrors {
    fn default() -> Self {
        Self {
            enabled: true,
            turns: 4,
            protected_tools: Vec::new(),
        }
    }
}

/// An inclusive span of message numbers in the raw session history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: usize,
    pub end: usize,
}

/// One model-authored compression. Later compressions have higher sequence
/// numbers and win over earlier ones where their ranges overlap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compression {
    pub seq: u64,
    pub ranges: Vec<Range>,
    pub summary: String,
}

/// Persisted pruning state, replayed from the session log.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DcpState {
    pub compressions: Vec<Compression>,
}

impl DcpState {
    fn next_seq(&self) -> u64 {
        self.compressions
            .iter()
            .map(|compression| compression.seq + 1)
            .max()
            .unwrap_or(0)
    }

    /// The highest message index covered by any recorded compression.
    fn last_compression_end(&self) -> Option<usize> {
        self.compressions
            .iter()
            .flat_map(|compression| compression.ranges.iter().map(|range| range.end))
            .max()
    }

    fn compression(&self, seq: u64) -> Option<&Compression> {
        self.compressions
            .iter()
            .find(|compression| compression.seq == seq)
    }
}

/// For each message index, the highest-sequence compression covering it.
/// Ranges are re-expanded against `messages` so a persisted compression that
/// ended mid tool-batch cannot orphan a tool result on replay.
fn active_seqs(messages: &[Message], state: &DcpState) -> Vec<Option<u64>> {
    let mut active: Vec<Option<u64>> = vec![None; messages.len()];
    for compression in &state.compressions {
        for range in normalize_ranges(messages, &compression.ranges) {
            for slot in active.iter_mut().take(range.end + 1).skip(range.start) {
                *slot = Some(match *slot {
                    Some(seq) => seq.max(compression.seq),
                    None => compression.seq,
                });
            }
        }
    }
    active
}

/// Loads DCP configuration for `cwd`, overlaying the project file on the
/// global one.
pub fn load_config(cwd: &Path) -> DcpConfig {
    let mut value = serde_json::to_value(DcpConfig::default()).unwrap_or(Value::Null);
    for path in config_paths(cwd) {
        if let Some(overlay) = read_json(&path) {
            merge(&mut value, &overlay);
        }
    }
    serde_json::from_value(value).unwrap_or_default()
}

fn config_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = dirs::config_dir() {
        paths.push(dir.join("oxide").join("dcp.json"));
    }
    if let Some(root) = project_root(cwd) {
        paths.push(root.join(".oxide").join("dcp.json"));
    }
    paths
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
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

/// A rough token estimate (about four characters per token).
pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| {
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
        })
        .sum()
}

fn is_protected(name: &str, cfg: &DcpConfig, extra: &[String]) -> bool {
    DEFAULT_PROTECTED_TOOLS.contains(&name)
        || cfg.protected_tools.iter().any(|tool| tool == name)
        || extra.iter().any(|tool| tool == name)
}

fn call_names(messages: &[Message]) -> HashMap<String, (String, String)> {
    let mut names = HashMap::new();
    for message in messages {
        if let Some(calls) = &message.tool_calls {
            for call in calls {
                names.insert(
                    call.id.clone(),
                    (call.function.name.clone(), call.function.arguments.clone()),
                );
            }
        }
    }
    names
}

fn tool_name<'a>(
    names: &'a HashMap<String, (String, String)>,
    message: &Message,
) -> Option<&'a str> {
    let id = message.tool_call_id.as_ref()?;
    names.get(id).map(|(name, _)| name.as_str())
}

fn file_protected(
    names: &HashMap<String, (String, String)>,
    message: &Message,
    cfg: &DcpConfig,
) -> bool {
    if cfg.protected_file_patterns.is_empty() {
        return false;
    }
    let Some((name, arguments)) = message.tool_call_id.as_ref().and_then(|id| names.get(id)) else {
        return false;
    };
    if !matches!(
        crate::tools::canonical_tool_name(name),
        "read_file" | "write_file" | "patch" | "edit"
    ) {
        return false;
    }
    let Ok(args) = serde_json::from_str::<Value>(arguments) else {
        return false;
    };
    let path = args.get("path").and_then(Value::as_str).unwrap_or("");
    cfg.protected_file_patterns
        .iter()
        .any(|pattern| wildcard_match(pattern, path))
}

/// Builds the pruned view sent to the model: compressions, then deduplication,
/// then errored-output purging.
pub fn prune(messages: &[Message], state: &DcpState, cfg: &DcpConfig) -> Vec<Message> {
    let active = active_seqs(messages, state);
    let mut out = Vec::with_capacity(messages.len());
    let mut previous: Option<u64> = None;
    for (index, message) in messages.iter().enumerate() {
        match active[index] {
            Some(seq) => {
                if previous != Some(seq) {
                    if let Some(compression) = state.compression(seq) {
                        out.push(Message::user(format!(
                            "[compressed conversation section]\n{}",
                            compression.summary
                        )));
                    }
                }
            }
            None => out.push(message.clone()),
        }
        previous = active[index];
    }

    if cfg.strategies.deduplication.enabled {
        out = deduplicate(out, cfg);
    }
    if cfg.strategies.purge_errors.enabled {
        out = purge_errors(out, cfg);
    }
    out
}

fn deduplicate(messages: Vec<Message>, cfg: &DcpConfig) -> Vec<Message> {
    let names = call_names(&messages);
    let extra = &cfg.strategies.deduplication.protected_tools;
    let mut latest: HashMap<(String, String), usize> = HashMap::new();
    for (index, message) in messages.iter().enumerate() {
        if message.role != "tool" {
            continue;
        }
        let Some(name) = tool_name(&names, message) else {
            continue;
        };
        if is_protected(name, cfg, extra) || file_protected(&names, message, cfg) {
            continue;
        }
        let Some((_, arguments)) = message.tool_call_id.as_ref().and_then(|id| names.get(id))
        else {
            continue;
        };
        latest.insert((name.to_string(), arguments.clone()), index);
    }

    let mut out = messages;
    for (index, message) in out.iter_mut().enumerate() {
        if message.role != "tool" {
            continue;
        }
        let Some(id) = message.tool_call_id.clone() else {
            continue;
        };
        let Some((name, arguments)) = names.get(&id) else {
            continue;
        };
        if is_protected(name, cfg, extra) || file_protected(&names, message, cfg) {
            continue;
        }
        let key = (name.clone(), arguments.clone());
        if latest.get(&key) == Some(&index) {
            continue;
        }
        let len = message.display().map(|text| text.len()).unwrap_or(0);
        if len < 120 || is_placeholder(message) {
            continue;
        }
        *message = Message::tool(id, "[duplicate tool result elided]");
    }
    out
}

fn purge_errors(messages: Vec<Message>, cfg: &DcpConfig) -> Vec<Message> {
    let names = call_names(&messages);
    let extra = &cfg.strategies.purge_errors.protected_tools;
    let turns = cfg.strategies.purge_errors.turns.max(1);
    let mut out = messages;
    let mut purge = vec![false; out.len()];
    for (index, message) in out.iter().enumerate() {
        if message.role != "tool" {
            continue;
        }
        let Some(text) = message.display() else {
            continue;
        };
        if !text.starts_with("error") || is_placeholder(message) {
            continue;
        }
        let Some(name) = tool_name(&names, message) else {
            continue;
        };
        if is_protected(name, cfg, extra) || file_protected(&names, message, cfg) {
            continue;
        }
        let users_after = out[index + 1..]
            .iter()
            .filter(|message| message.role == "user")
            .count();
        if users_after < turns {
            continue;
        }
        purge[index] = true;
    }
    for (index, message) in out.iter_mut().enumerate() {
        if !purge[index] {
            continue;
        }
        if let Some(id) = message.tool_call_id.clone() {
            *message = Message::tool(id, "[errored tool output purged]");
        }
    }
    out
}

fn is_placeholder(message: &Message) -> bool {
    message.display().is_some_and(|text| {
        text.starts_with("[duplicate tool result elided]")
            || text.starts_with("[errored tool output purged]")
    })
}

/// A compact numbered listing of the raw history, including which spans are
/// already compressed. Injected as a nudge so the model can choose ranges.
pub fn context_index(messages: &[Message], state: &DcpState) -> String {
    let active = active_seqs(messages, state);
    let mut out = String::from("Conversation index (message numbers for the `compress` tool):\n");
    let mut index = 0;
    while index < messages.len() {
        match active[index] {
            Some(seq) => {
                let start = index;
                while index < messages.len() && active[index] == Some(seq) {
                    index += 1;
                }
                out.push_str(&format!("#{start}-{} [compressed]\n", index - 1));
            }
            None => {
                let preview: String = messages[index]
                    .display()
                    .unwrap_or_default()
                    .replace('\n', " ")
                    .chars()
                    .take(80)
                    .collect();
                out.push_str(&format!("#{index} {}: {preview}\n", messages[index].role));
                index += 1;
            }
        }
    }
    out
}

/// Returns a compression nudge when the estimated context is large enough,
/// otherwise `None`. `iterations` counts model iterations since the last user
/// message.
pub fn nudge(
    messages: &[Message],
    state: &DcpState,
    cfg: &DcpConfig,
    iterations: usize,
) -> Option<String> {
    if !cfg.enabled {
        return None;
    }
    if let Some(last_end) = state.last_compression_end() {
        if messages.len().saturating_sub(last_end + 1) < cfg.compress.cooldown_messages {
            return None;
        }
    }
    let tokens = estimate_tokens(messages);
    let over_max = tokens >= cfg.compress.max_context_limit;
    let over_min = tokens >= cfg.compress.min_context_limit;
    let frequency_due =
        cfg.compress.nudge_frequency > 0 && iterations.is_multiple_of(cfg.compress.nudge_frequency);
    let iteration_due = iterations >= cfg.compress.iteration_nudge_threshold;
    if !(over_max || (over_min && (frequency_due || iteration_due))) {
        return None;
    }
    let index = context_index(messages, state);
    Some(format!(
        "Context is large (~{tokens} estimated tokens). If earlier work is complete, call the \
         `compress` tool to replace closed spans with concise summaries. Session history is not \
         modified. Use these message numbers:\n\n{index}"
    ))
}

/// The `compress` tool spec, or `None` when pruning is disabled or compression
/// is denied.
pub fn compress_spec(cfg: &DcpConfig) -> Option<ToolSpec> {
    if !cfg.enabled || cfg.compress.permission == "deny" {
        return None;
    }
    Some(ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "compress".to_string(),
            description: "Replace closed, stale spans of the conversation with a concise technical \
                          summary, reducing context usage. Use it when earlier work is complete and \
                          no longer needs to be kept verbatim. Provide one or more `ranges` of \
                          message numbers from the context index and a `summary` preserving goals, \
                          decisions, file paths, code changes and unresolved tasks. Session history \
                          is not modified."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "ranges": {
                        "type": "array",
                        "description": "Inclusive message-number ranges to compress",
                        "items": {
                            "type": "object",
                            "properties": {
                                "start": { "type": "integer", "description": "First message number" },
                                "end": { "type": "integer", "description": "Last message number" }
                            },
                            "required": ["start", "end"]
                        }
                    },
                    "summary": {
                        "type": "string",
                        "description": "High-fidelity summary preserving everything important"
                    }
                },
                "required": ["ranges", "summary"]
            }),
        },
    })
}

/// Applies a model `compress` call to `state`, returning a human-readable
/// result. Ranges are normalized so tool calls and their results stay paired,
/// and the most recent message is never compressed.
pub fn apply_compress(
    state: &mut DcpState,
    messages: &[Message],
    cfg: &DcpConfig,
    args: &Value,
) -> Result<String> {
    let summary = args
        .get("summary")
        .and_then(Value::as_str)
        .context("missing `summary` argument")?
        .trim()
        .to_string();
    if summary.is_empty() {
        bail!("`summary` must not be empty");
    }
    let raw = args
        .get("ranges")
        .and_then(Value::as_array)
        .context("missing `ranges` array")?;
    if raw.is_empty() {
        bail!("`ranges` must not be empty");
    }

    let mut ranges = Vec::with_capacity(raw.len());
    for range in raw {
        let start = range
            .get("start")
            .and_then(Value::as_u64)
            .context("range missing `start`")? as usize;
        let end = range
            .get("end")
            .and_then(Value::as_u64)
            .context("range missing `end`")? as usize;
        if start > end {
            bail!("range start {start} is after end {end}");
        }
        if end >= messages.len() {
            bail!(
                "range end {end} is out of bounds (history has {} messages)",
                messages.len()
            );
        }
        ranges.push(Range { start, end });
    }

    let limit = messages
        .len()
        .saturating_sub(cfg.compress.protected_recent_messages.max(1));
    let mut normalized = normalize_ranges(messages, &ranges);
    for range in &mut normalized {
        if range.end >= limit {
            range.end = limit.saturating_sub(1);
        }
    }
    normalized.retain(|range| range.start < limit && range.start <= range.end);
    if normalized.is_empty() {
        bail!(
            "no compressible ranges; leave the most recent {} messages intact",
            cfg.compress.protected_recent_messages
        );
    }

    let mut summary = summary;
    let retained = retained_protected(messages, &normalized, cfg);
    if !retained.is_empty() {
        summary.push_str("\n\nProtected outputs retained:\n");
        summary.push_str(&retained.join("\n"));
    }

    let covered: usize = normalized
        .iter()
        .map(|range| range.end - range.start + 1)
        .sum();
    let seq = state.next_seq();
    state.compressions.push(Compression {
        seq,
        ranges: normalized.clone(),
        summary,
    });
    Ok(format!(
        "compressed {covered} messages into {} summary range(s)",
        normalized.len()
    ))
}

fn retained_protected(messages: &[Message], ranges: &[Range], cfg: &DcpConfig) -> Vec<String> {
    let names = call_names(messages);
    let mut out = Vec::new();
    for range in ranges {
        for message in &messages[range.start..=range.end] {
            if message.role != "tool" {
                continue;
            }
            let Some(name) = tool_name(&names, message) else {
                continue;
            };
            if !cfg.compress.protected_tools.iter().any(|tool| tool == name) {
                continue;
            }
            if let Some(text) = message.display() {
                let preview: String = text.chars().take(500).collect();
                out.push(format!("- {name}: {preview}"));
            }
        }
    }
    out
}

fn normalize_ranges(messages: &[Message], ranges: &[Range]) -> Vec<Range> {
    let mut clamped: Vec<Range> = ranges
        .iter()
        .filter(|range| range.start < messages.len())
        .map(|range| Range {
            start: range.start,
            end: range.end.min(messages.len() - 1),
        })
        .collect();
    clamped.sort_by_key(|range| (range.start, range.end));

    let mut merged: Vec<Range> = Vec::new();
    for range in clamped {
        if let Some(last) = merged.last_mut() {
            if range.start <= last.end + 1 {
                last.end = last.end.max(range.end);
                continue;
            }
        }
        merged.push(range);
    }
    merged
        .into_iter()
        .map(|range| expand_pair(messages, range))
        .collect()
}

fn expand_pair(messages: &[Message], range: Range) -> Range {
    let mut start = range.start;
    while start > 0 && messages[start].role == "tool" {
        start -= 1;
    }
    let mut end = range.end;
    let mut index = start;
    while index <= end && index < messages.len() {
        if let Some(calls) = messages[index].tool_calls.as_ref() {
            let ids: HashSet<&str> = calls.iter().map(|call| call.id.as_str()).collect();
            while end + 1 < messages.len() && messages[end + 1].role == "tool" {
                match messages[end + 1].tool_call_id.as_deref() {
                    Some(id) if ids.contains(id) => end += 1,
                    _ => break,
                }
            }
        }
        index += 1;
    }
    Range { start, end }
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    fn helper(pattern: &[char], text: &[char]) -> bool {
        if pattern.is_empty() {
            return text.is_empty();
        }
        match pattern[0] {
            '*' => helper(&pattern[1..], text) || (!text.is_empty() && helper(pattern, &text[1..])),
            '?' => !text.is_empty() && helper(&pattern[1..], &text[1..]),
            ch => !text.is_empty() && text[0] == ch && helper(&pattern[1..], &text[1..]),
        }
    }
    helper(
        &pattern.chars().collect::<Vec<_>>(),
        &text.chars().collect::<Vec<_>>(),
    )
}

/// Builds an assistant message carrying tool calls, used by tests and helpers.
#[cfg(test)]
fn tool_call(id: &str, name: &str, arguments: &str) -> Message {
    Message::assistant(
        "",
        vec![crate::llm::ToolCall {
            id: id.to_string(),
            kind: "function".to_string(),
            function: crate::llm::FunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_enabled_and_bounded() {
        let cfg = DcpConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.strategies.deduplication.enabled);
        assert!(cfg.compress.min_context_limit < cfg.compress.max_context_limit);
    }

    #[test]
    fn token_estimate_includes_tool_arguments() {
        let messages = vec![tool_call(
            "call-1",
            "write_file",
            &format!(r#"{{"path":"x","content":"{}"}}"#, "x".repeat(400)),
        )];
        assert!(estimate_tokens(&messages) >= 100);
    }

    #[test]
    fn compression_replaces_range_with_placeholder() {
        let messages = vec![
            Message::user("first"),
            Message::assistant("reply", vec![]),
            Message::user("second"),
        ];
        let state = DcpState {
            compressions: vec![Compression {
                seq: 0,
                ranges: vec![Range { start: 0, end: 1 }],
                summary: "did the first thing".into(),
            }],
        };
        let pruned = prune(&messages, &state, &DcpConfig::default());
        assert_eq!(pruned.len(), 2);
        assert!(pruned[0].display().unwrap().contains("did the first thing"));
        assert_eq!(pruned[1].display().as_deref(), Some("second"));
    }

    #[test]
    fn deduplication_keeps_latest_output() {
        let long = "x".repeat(200);
        let messages = vec![
            tool_call("a", "read_file", r#"{"path":"src/a.rs"}"#),
            Message::tool("a", long.clone()),
            tool_call("b", "read_file", r#"{"path":"src/a.rs"}"#),
            Message::tool("b", long),
        ];
        let pruned = prune(&messages, &DcpState::default(), &DcpConfig::default());
        assert_eq!(
            pruned[1].display().as_deref(),
            Some("[duplicate tool result elided]")
        );
        assert!(pruned[3].display().unwrap().len() > 100);
    }

    #[test]
    fn purge_errors_waits_for_turns() {
        let mut messages = vec![
            tool_call("a", "bash", r#"{"command":"false"}"#),
            Message::tool("a", "error: command failed"),
        ];
        for index in 0..4 {
            messages.push(Message::user(format!("turn {index}")));
        }
        let pruned = prune(&messages, &DcpState::default(), &DcpConfig::default());
        assert_eq!(
            pruned[1].display().as_deref(),
            Some("[errored tool output purged]")
        );
    }

    #[test]
    fn normalize_ranges_keeps_tool_pairs_together() {
        let messages = vec![
            Message::user("q"),
            tool_call("a", "read_file", r#"{"path":"x"}"#),
            Message::tool("a", "contents"),
            Message::user("next"),
        ];
        let normalized = normalize_ranges(&messages, &[Range { start: 2, end: 2 }]);
        assert_eq!(normalized, vec![Range { start: 1, end: 2 }]);
    }

    #[test]
    fn normalize_ranges_covers_tool_results_after_an_intermediate_batch() {
        let messages = vec![
            Message::user("q"),
            tool_call("a", "read_file", r#"{"path":"x"}"#),
            Message::tool("a", "a-result"),
            tool_call("b", "grep", r#"{"pattern":"y"}"#),
            Message::tool("b", "b-result"),
            tool_call("c", "read_file", r#"{"path":"y"}"#),
            Message::tool("c", "c-result"),
        ];
        let normalized = normalize_ranges(&messages, &[Range { start: 2, end: 5 }]);
        assert_eq!(normalized, vec![Range { start: 1, end: 6 }]);
    }

    fn tool_pairs_are_consistent(messages: &[Message]) -> bool {
        let mut pending: HashSet<String> = HashSet::new();
        for message in messages {
            match message.role.as_str() {
                "assistant" => {
                    pending.clear();
                    if let Some(calls) = &message.tool_calls {
                        for call in calls {
                            pending.insert(call.id.clone());
                        }
                    }
                }
                "tool" => {
                    let Some(id) = message.tool_call_id.as_ref() else {
                        return false;
                    };
                    if !pending.remove(id) {
                        return false;
                    }
                }
                _ => pending.clear(),
            }
        }
        true
    }

    #[test]
    fn prune_never_orphans_tool_results() {
        let messages = vec![
            Message::user("q"),
            tool_call("a", "read_file", r#"{"path":"x"}"#),
            Message::tool("a", "a-result"),
            tool_call("b", "grep", r#"{"pattern":"y"}"#),
            Message::tool("b", "b-result"),
            tool_call("c", "read_file", r#"{"path":"y"}"#),
            Message::tool("c", "c-result"),
        ];
        let state = DcpState {
            compressions: vec![Compression {
                seq: 0,
                ranges: vec![Range { start: 2, end: 5 }],
                summary: "compressed".into(),
            }],
        };
        let pruned = prune(&messages, &state, &DcpConfig::default());
        assert!(tool_pairs_are_consistent(&pruned));
    }

    #[test]
    fn apply_compress_rejects_out_of_bounds_and_recent() {
        let messages = vec![Message::user("only")];
        let mut state = DcpState::default();
        let cfg = DcpConfig::default();
        let args = json!({"ranges":[{"start":0,"end":5}],"summary":"s"});
        assert!(apply_compress(&mut state, &messages, &cfg, &args).is_err());
        let args = json!({"ranges":[{"start":0,"end":0}],"summary":"s"});
        assert!(apply_compress(&mut state, &messages, &cfg, &args).is_err());
        assert!(state.compressions.is_empty());
    }

    #[test]
    fn apply_compress_records_range() {
        let mut messages = vec![Message::user("a"), Message::assistant("b", vec![])];
        for index in 0..10 {
            messages.push(Message::user(format!("m{index}")));
        }
        let mut state = DcpState::default();
        let cfg = DcpConfig::default();
        let args = json!({"ranges":[{"start":0,"end":1}],"summary":"summary"});
        let text = apply_compress(&mut state, &messages, &cfg, &args).unwrap();
        assert!(text.starts_with("compressed 2 messages"));
        assert_eq!(state.compressions.len(), 1);
    }

    #[test]
    fn apply_compress_protects_recent_messages() {
        let messages: Vec<Message> = (0..12).map(|i| Message::user(format!("m{i}"))).collect();
        let mut state = DcpState::default();
        let cfg = DcpConfig::default();
        let args = json!({"ranges":[{"start":0,"end":11}],"summary":"s"});
        apply_compress(&mut state, &messages, &cfg, &args).unwrap();
        let stored = state.compressions[0].ranges[0];
        assert!(stored.end < messages.len() - cfg.compress.protected_recent_messages);
    }

    #[test]
    fn nudge_respects_compression_cooldown() {
        let mut messages = vec![Message::user("x".repeat(200_000))];
        let cfg = DcpConfig::default();
        let mut state = DcpState::default();
        assert!(nudge(&messages, &state, &cfg, 0).is_some());

        state.compressions.push(Compression {
            seq: 0,
            ranges: vec![Range { start: 0, end: 0 }],
            summary: "s".into(),
        });
        assert!(nudge(&messages, &state, &cfg, 0).is_none());

        for index in 0..cfg.compress.cooldown_messages {
            messages.push(Message::user(format!("m{index}")));
        }
        assert!(nudge(&messages, &state, &cfg, 0).is_some());
    }

    #[test]
    fn context_index_marks_compressed_spans() {
        let messages = vec![Message::user("a"), Message::assistant("b", vec![])];
        let state = DcpState {
            compressions: vec![Compression {
                seq: 0,
                ranges: vec![Range { start: 0, end: 0 }],
                summary: "s".into(),
            }],
        };
        let index = context_index(&messages, &state);
        assert!(index.contains("#0-0 [compressed]"));
        assert!(index.contains("#1 assistant"));
    }

    #[test]
    fn nudge_fires_above_max_limit() {
        let messages = vec![Message::user("x".repeat(200_000))];
        let cfg = DcpConfig::default();
        assert!(nudge(&messages, &DcpState::default(), &cfg, 0).is_some());
    }

    #[test]
    fn disabled_config_disables_spec_and_nudge() {
        let cfg = DcpConfig {
            enabled: false,
            ..DcpConfig::default()
        };
        assert!(compress_spec(&cfg).is_none());
        assert!(nudge(
            &[Message::user("x".repeat(200_000))],
            &DcpState::default(),
            &cfg,
            0
        )
        .is_none());
    }

    #[test]
    fn merge_overlays_nested_objects() {
        let mut base = json!({"a": 1, "nested": {"x": 1, "y": 2}});
        merge(&mut base, &json!({"nested": {"y": 3}, "b": 4}));
        assert_eq!(base["nested"]["y"], 3);
        assert_eq!(base["nested"]["x"], 1);
        assert_eq!(base["b"], 4);
        assert_eq!(base["a"], 1);
    }

    #[test]
    fn config_parses_camel_case_overrides_with_defaults() {
        let value = json!({
            "enabled": false,
            "compress": {"maxContextLimit": 1234},
            "strategies": {"purgeErrors": {"turns": 2}}
        });
        let cfg: DcpConfig = serde_json::from_value(value).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.compress.max_context_limit, 1234);
        assert_eq!(cfg.compress.min_context_limit, 16_000);
        assert_eq!(cfg.strategies.purge_errors.turns, 2);
        assert!(cfg.strategies.deduplication.enabled);
    }
}
