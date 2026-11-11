use crate::llm::{ContentPart, ImageUrl, Message, MessageContent, Usage};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TokenEfficiency {
    pub reuse_reads: bool,
    pub max_declared_mcp_tools: usize,
    pub tool_history: ToolHistory,
    pub turn_budget: BudgetLimits,
}

impl Default for TokenEfficiency {
    fn default() -> Self {
        Self {
            reuse_reads: true,
            max_declared_mcp_tools: 24,
            tool_history: ToolHistory::default(),
            turn_budget: BudgetLimits::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolHistory {
    pub enabled: bool,
    pub prune_above_tokens: usize,
    pub keep_recent: usize,
    pub max_result_bytes: usize,
    pub keep_recent_images: usize,
}

impl Default for ToolHistory {
    fn default() -> Self {
        Self {
            enabled: true,
            prune_above_tokens: 32_000,
            keep_recent: 8,
            max_result_bytes: 2048,
            keep_recent_images: 8,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BudgetLimits {
    pub max_model_calls: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub max_cost_usd: Option<f64>,
}

pub fn load(cwd: &Path) -> Result<TokenEfficiency> {
    let mut paths = vec![crate::config::settings_path()];
    if let Some(root) = crate::ecosystem::project_root(cwd) {
        paths.push(root.join(".oxide/settings.json"));
    }
    load_paths(&paths)
}

fn load_paths(paths: &[PathBuf]) -> Result<TokenEfficiency> {
    let mut value = serde_json::to_value(TokenEfficiency::default())?;
    for path in paths {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        let settings: Value = serde_json::from_str(&text)
            .with_context(|| format!("reading settings from {}", path.display()))?;
        if let Some(overlay) = settings.get("tokenEfficiency") {
            merge(&mut value, overlay);
        }
    }
    let config: TokenEfficiency =
        serde_json::from_value(value).context("invalid tokenEfficiency settings")?;
    anyhow::ensure!(
        (1..=64).contains(&config.max_declared_mcp_tools),
        "tokenEfficiency.maxDeclaredMcpTools must be between 1 and 64"
    );
    anyhow::ensure!(
        config.tool_history.keep_recent > 0
            && config.tool_history.max_result_bytes >= 256
            && config.tool_history.prune_above_tokens > 0,
        "tokenEfficiency.toolHistory requires positive keepRecent/pruneAboveTokens and maxResultBytes >= 256"
    );
    anyhow::ensure!(
        config.turn_budget.max_model_calls != Some(0)
            && config.turn_budget.max_output_tokens != Some(0)
            && config
                .turn_budget
                .max_cost_usd
                .is_none_or(|cost| cost.is_finite() && cost > 0.0),
        "tokenEfficiency.turnBudget limits must be positive and finite (or null for unlimited)"
    );
    Ok(config)
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

#[derive(Debug, Default)]
pub struct TurnBudget {
    limits: BudgetLimits,
    calls: u64,
    output: u64,
    cost: f64,
    reserved_output: u64,
}

impl TurnBudget {
    pub fn new(limits: BudgetLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    pub fn reason(&self) -> Option<BudgetExceeded> {
        let limit = if self
            .limits
            .max_model_calls
            .is_some_and(|cap| self.calls >= cap)
        {
            "model-call"
        } else if self
            .limits
            .max_output_tokens
            .is_some_and(|cap| self.output >= cap)
        {
            "output-token"
        } else if self.limits.max_cost_usd.is_some_and(|cap| self.cost >= cap) {
            "estimated-cost"
        } else {
            return None;
        };
        Some(BudgetExceeded(format!(
            "Run paused at its {limit} budget: {} model calls, {} output tokens, ${:.4} estimated cost. Work is saved; send another message to continue.",
            self.calls, self.output, self.cost
        )))
    }
}

#[derive(Debug)]
pub struct BudgetExceeded(pub String);

impl fmt::Display for BudgetExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BudgetExceeded {}

#[derive(Debug)]
pub(crate) struct RequestPermit {
    budget: Arc<Mutex<TurnBudget>>,
    pub max_tokens: u32,
}

impl RequestPermit {
    pub fn acquire(budget: &Arc<Mutex<TurnBudget>>, requested: u32) -> Result<Self> {
        let mut state = budget.lock().expect("turn budget poisoned");
        if let Some(reason) = state.reason() {
            return Err(reason.into());
        }
        let remaining = state
            .limits
            .max_output_tokens
            .map_or(u64::from(requested), |cap| {
                cap.saturating_sub(state.output)
                    .saturating_sub(state.reserved_output)
            });
        anyhow::ensure!(
            remaining > 0,
            BudgetExceeded(
                "Run output-token budget is reserved by another request; work is saved.".into()
            )
        );
        let max_tokens = u64::from(requested).min(remaining) as u32;
        state.calls += 1;
        state.reserved_output += u64::from(max_tokens);
        drop(state);
        Ok(Self {
            budget: Arc::clone(budget),
            max_tokens,
        })
    }

    pub fn record(&self, usage: Usage, cost: f64) {
        let mut state = self.budget.lock().expect("turn budget poisoned");
        state.output += usage.output;
        state.cost += cost;
    }
}

impl Drop for RequestPermit {
    fn drop(&mut self) {
        let mut state = self.budget.lock().expect("turn budget poisoned");
        state.reserved_output = state
            .reserved_output
            .saturating_sub(u64::from(self.max_tokens));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadVersion {
    key: String,
    digest: [u8; 32],
}

impl ReadVersion {
    pub fn capture(cwd: &Path, arguments: &str) -> Option<Self> {
        let mut args: Value = serde_json::from_str(arguments).ok()?;
        let object = args.as_object_mut()?;
        object.remove("fresh");
        object.entry("offset").or_insert(Value::from(1));
        object.entry("limit").or_insert(Value::from(2000));
        let path = object.get("path")?.as_str()?;
        let full = cwd.join(path).canonicalize().ok()?;
        if crate::media::is_attachment_path(&full) {
            return None;
        }
        let metadata = std::fs::metadata(&full).ok()?;
        if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
            return None;
        }
        let mut file = std::fs::File::open(&full).ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > 8 * 1024 * 1024 {
            return None;
        }
        object.insert(
            "path".into(),
            Value::String(full.to_string_lossy().into_owned()),
        );
        Some(Self {
            key: serde_json::to_string(&args).ok()?,
            digest: Sha256::digest(&bytes).into(),
        })
    }
}

#[derive(Default)]
pub(crate) struct ReadReuse {
    entries: BTreeMap<String, (ReadVersion, String)>,
}

impl ReadReuse {
    pub fn previous(&self, version: &ReadVersion) -> Option<&str> {
        self.entries
            .get(&version.key)
            .filter(|(old, _)| old == version)
            .map(|(_, id)| id.as_str())
    }

    pub fn remember(&mut self, version: ReadVersion, id: &str) {
        if self.entries.len() >= 128 {
            self.entries.clear();
        }
        self.entries
            .insert(version.key.clone(), (version, id.to_string()));
    }

    pub fn retain_context(&mut self, messages: &[Message]) {
        let available: HashSet<&str> = messages
            .iter()
            .filter(|message| {
                message.role == "tool"
                    && !message.display().is_some_and(|text| {
                        text.starts_with("[tool output compacted;") || text.starts_with("error:")
                    })
            })
            .filter_map(|message| message.tool_call_id.as_deref())
            .collect();
        self.entries
            .retain(|_, (_, id)| available.contains(id.as_str()));
    }

    /// Seeds this memory from reads already in the conversation, so a
    /// follow-up turn points at a result it can still see instead of sending
    /// the same file again: this memory lives for one run, while the
    /// conversation outlives it.
    ///
    /// `render` re-runs a recorded call. An entry is kept only when the call
    /// renders exactly the text the conversation holds, so a file edited since
    /// that read is never offered for reuse.
    pub fn seed(
        &mut self,
        cwd: &Path,
        reads: &[RecordedRead],
        render: impl Fn(&str) -> Option<String>,
    ) -> usize {
        let mut seeded = 0;
        for read in reads {
            let Some(version) = ReadVersion::capture(cwd, &read.arguments) else {
                continue;
            };
            // `reads` is newest first, so an older read of the same range is
            // already represented by the one kept here.
            if self.entries.contains_key(&version.key) {
                continue;
            }
            if render(&read.arguments).as_deref() != Some(read.text.as_str()) {
                continue;
            }
            self.remember(version, &read.id);
            seeded += 1;
        }
        seeded
    }
}

/// A `read` result still present in the conversation, with the call that
/// produced it.
pub(crate) struct RecordedRead {
    pub id: String,
    pub arguments: String,
    pub text: String,
}

/// The `read` results in `messages` whose content a later turn could reuse,
/// newest first and bounded. A compacted, failed or reuse-pointer result is
/// skipped: its content is not in the conversation, so re-reading the file is
/// the only way to see it.
pub(crate) fn recorded_reads(messages: &[Message], limit: usize) -> Vec<RecordedRead> {
    let mut arguments: BTreeMap<&str, &str> = BTreeMap::new();
    for message in messages {
        for call in message.tool_calls.iter().flatten() {
            if call.function.name == "read" {
                arguments.insert(&call.id, &call.function.arguments);
            }
        }
    }
    if arguments.is_empty() {
        return Vec::new();
    }
    let mut reads = Vec::new();
    for message in messages.iter().rev() {
        if reads.len() >= limit {
            break;
        }
        if message.role != "tool" {
            continue;
        }
        let Some(id) = message.tool_call_id.as_deref() else {
            continue;
        };
        let Some(call) = arguments.get(id).copied() else {
            continue;
        };
        let Some(text) = message.display() else {
            continue;
        };
        if text.is_empty()
            || text.starts_with("[tool output compacted;")
            || text.starts_with("error:")
            || text.starts_with("Unchanged file and range;")
        {
            continue;
        }
        reads.push(RecordedRead {
            id: id.to_string(),
            arguments: call.to_string(),
            text,
        });
    }
    reads
}

/// Only reclaim from the measured prefix; new results already shrink in the growth estimate.
pub(crate) fn prune_tool_history(
    messages: &mut [Message],
    measured_at: usize,
    config: &ToolHistory,
    mut save: impl FnMut(&str) -> Option<PathBuf>,
) -> usize {
    if !config.enabled || crate::compact::estimate_tokens(messages) < config.prune_above_tokens {
        return 0;
    }
    let recent = messages
        .iter()
        .filter(|m| m.role == "tool")
        .count()
        .saturating_sub(config.keep_recent);
    let measured_at = measured_at.min(messages.len());
    let before = crate::compact::estimate_tokens(&messages[..measured_at]);
    let mut seen = 0;
    for message in messages.iter_mut().filter(|m| m.role == "tool") {
        seen += 1;
        if seen > recent {
            break;
        }
        let Some(MessageContent::Text(text)) = &mut message.content else {
            continue;
        };
        if text.len() <= config.max_result_bytes || text.starts_with("[tool output compacted;") {
            continue;
        }
        let Some(path) = save(text) else {
            continue;
        };
        let half = config.max_result_bytes / 2;
        let mut head = half.min(text.len());
        while !text.is_char_boundary(head) {
            head -= 1;
        }
        let mut tail = text.len().saturating_sub(half);
        while !text.is_char_boundary(tail) {
            tail += 1;
        }
        let status = if text.starts_with("error:") || failed_shell(text) {
            " original tool failed;"
        } else {
            ""
        };
        let shortened = format!("[tool output compacted;{status} full: {}. Read this file for omitted evidence.]\n{}\n… omitted stale output …\n{}", path.display(), &text[..head], &text[tail..]);
        if shortened.len() < text.len() {
            *text = shortened;
        }
    }
    before.saturating_sub(crate::compact::estimate_tokens(&messages[..measured_at]))
}

fn failed_shell(text: &str) -> bool {
    text.lines()
        .rev()
        .find_map(|line| {
            line.strip_prefix("[exit: ")
                .or_else(|| line.strip_prefix("[exit code: "))
                .and_then(|value| value.strip_suffix(']'))
                .and_then(|value| value.parse::<i32>().ok())
        })
        .is_some_and(|code| code != 0)
}

pub(crate) fn prune_tool_images(
    messages: &mut [Message],
    seen_through: usize,
    config: &ToolHistory,
    mut save: impl FnMut(&ImageUrl) -> Option<PathBuf>,
) -> usize {
    let keep = config.keep_recent_images;
    if !config.enabled || keep == 0 {
        return 0;
    }
    let count = messages
        .iter()
        .filter(|message| message.role == "tool")
        .filter_map(|message| match &message.content {
            Some(MessageContent::Parts(parts)) => Some(parts),
            _ => None,
        })
        .flatten()
        .filter(|part| matches!(part, ContentPart::ImageUrl { .. }))
        .count();
    // Rewriting an early result invalidates the cached prefix after it. Reclaim
    // a batch instead of moving that boundary on every new screenshot.
    if count <= keep.saturating_mul(2) {
        return 0;
    }
    let mut remaining = count - keep;
    let mut removed = 0;
    for message in messages
        .iter_mut()
        .take(seen_through)
        .filter(|message| message.role == "tool")
    {
        let Some(MessageContent::Parts(parts)) = &mut message.content else {
            continue;
        };
        for part in parts {
            if remaining == 0 {
                return removed;
            }
            let ContentPart::ImageUrl { image_url } = part else {
                continue;
            };
            remaining -= 1;
            if let Some(path) = save(image_url) {
                *part = ContentPart::Text {
                    text: format!(
                        "[older tool image saved: {}. Use read on this path to inspect it again.]",
                        path.display()
                    ),
                };
                removed += 1;
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oxide_efficiency_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tool_call(id: &str, name: &str, arguments: &str) -> crate::llm::ToolCall {
        crate::llm::ToolCall {
            id: id.to_string(),
            kind: "function".to_string(),
            function: crate::llm::FunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
            signature: None,
        }
    }

    #[test]
    fn settings_merge_per_key_and_reject_invalid_limits() {
        let dir = temp_dir();
        let global = dir.join("global.json");
        let project = dir.join("project.json");
        std::fs::write(
            &global,
            json!({"tokenEfficiency": {
                "reuseReads": false, "turnBudget": {"maxModelCalls": 10, "maxCostUsd": 0.5}
            }})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            &project,
            json!({"tokenEfficiency": {
                "turnBudget": {"maxModelCalls": 4}, "toolHistory": {"keepRecent": 3}
            }})
            .to_string(),
        )
        .unwrap();
        let config = load_paths(&[global.clone(), project.clone()]).unwrap();
        assert!(!config.reuse_reads);
        assert_eq!(config.turn_budget.max_model_calls, Some(4));
        assert_eq!(config.turn_budget.max_cost_usd, Some(0.5));
        assert_eq!(config.tool_history.keep_recent, 3);
        assert_eq!(config.tool_history.max_result_bytes, 2048);
        for invalid in [
            json!({"turnBudget":{"maxModelCalls":0}}),
            json!({"turnBudget":{"maxCostUsd":-1}}),
            json!({"maxDeclaredMcpTools":0}),
            json!({"toolHistory":{"keepRecent":0}}),
        ] {
            std::fs::write(&project, json!({"tokenEfficiency":invalid}).to_string()).unwrap();
            assert!(load_paths(std::slice::from_ref(&project)).is_err());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn budgets_reserve_shared_output_and_count_failed_attempts() {
        let budget = Arc::new(Mutex::new(TurnBudget::new(BudgetLimits {
            max_model_calls: Some(3),
            max_output_tokens: Some(100),
            ..Default::default()
        })));
        let first = RequestPermit::acquire(&budget, 80).unwrap();
        let second = RequestPermit::acquire(&budget, 80).unwrap();
        assert_eq!(second.max_tokens, 20);
        assert!(RequestPermit::acquire(&budget, 1)
            .unwrap_err()
            .is::<BudgetExceeded>());
        first.record(
            Usage {
                output: 70,
                ..Default::default()
            },
            0.0,
        );
        drop(first);
        drop(second);
        // The failed second provider attempt still counts as a call, but releases its reservation.
        let third = RequestPermit::acquire(&budget, 80).unwrap();
        assert_eq!(third.max_tokens, 30);
        third.record(
            Usage {
                output: 30,
                ..Default::default()
            },
            0.0,
        );
        drop(third);
        assert!(RequestPermit::acquire(&budget, 1)
            .unwrap_err()
            .is::<BudgetExceeded>());
    }

    #[test]
    fn cost_and_output_thresholds_stop_subsequent_requests() {
        for limits in [
            BudgetLimits {
                max_output_tokens: Some(10),
                ..Default::default()
            },
            BudgetLimits {
                max_cost_usd: Some(0.1),
                ..Default::default()
            },
        ] {
            let budget = Arc::new(Mutex::new(TurnBudget::new(limits)));
            let request = RequestPermit::acquire(&budget, 100).unwrap();
            request.record(
                Usage {
                    output: 10,
                    ..Default::default()
                },
                0.1,
            );
            drop(request);
            assert!(RequestPermit::acquire(&budget, 100).is_err());
        }
    }

    #[test]
    fn read_versions_follow_content_range_and_context_availability() {
        let dir = temp_dir();
        let path = dir.join("file.txt");
        std::fs::write(&path, "hello").unwrap();
        let args = json!({"path":path,"offset":1,"limit":1}).to_string();
        let first = ReadVersion::capture(&dir, &args).unwrap();
        let mut cache = ReadReuse::default();
        cache.remember(first.clone(), "first");
        assert_eq!(
            cache.previous(&ReadVersion::capture(&dir, &args).unwrap()),
            Some("first")
        );
        std::fs::write(&path, "world").unwrap();
        assert!(cache
            .previous(&ReadVersion::capture(&dir, &args).unwrap())
            .is_none());
        std::fs::write(&path, "hello").unwrap();
        let other_range = json!({"path":path,"offset":2,"limit":1}).to_string();
        assert!(cache
            .previous(&ReadVersion::capture(&dir, &other_range).unwrap())
            .is_none());
        cache.retain_context(&[Message::tool("first", "original content")]);
        assert!(cache.previous(&first).is_some());
        cache.retain_context(&[Message::tool(
            "first",
            "[tool output compacted; full: /tmp/output]",
        )]);
        assert!(cache.previous(&first).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A later turn starts with an empty read memory while the conversation
    /// still holds the earlier results, so the memory is seeded from them —
    /// but only for a file that still renders exactly what the conversation
    /// shows.
    #[test]
    fn seeding_reuses_reads_from_the_conversation_only_while_they_still_match() {
        let dir = temp_dir();
        let path = dir.join("file.txt");
        std::fs::write(&path, "hello").unwrap();
        let stale = dir.join("stale.txt");
        std::fs::write(&stale, "edited since").unwrap();
        let args = json!({"path":path}).to_string();
        let stale_args = json!({"path":stale}).to_string();
        let messages = vec![
            Message::assistant(
                "reading",
                vec![
                    tool_call("fresh", "read", &args),
                    tool_call("old", "read", &stale_args),
                    tool_call("compacted", "read", &args),
                    tool_call("shell", "bash", &json!({"command":"ls"}).to_string()),
                ],
            ),
            Message::tool("fresh", "1|hello"),
            Message::tool("old", "1|before the edit"),
            Message::tool("compacted", "[tool output compacted; full: /tmp/output]"),
            Message::tool("shell", "file.txt"),
        ];
        let reads = recorded_reads(&messages, 64);
        assert_eq!(reads.len(), 2, "only uncompacted read results");
        let mut cache = ReadReuse::default();
        let rendered = |arguments: &str| {
            let path = serde_json::from_str::<Value>(arguments).ok()?["path"]
                .as_str()?
                .to_string();
            Some(format!("1|{}", std::fs::read_to_string(path).ok()?))
        };
        assert_eq!(cache.seed(&dir, &reads, rendered), 1);
        assert_eq!(
            cache.previous(&ReadVersion::capture(&dir, &args).unwrap()),
            Some("fresh")
        );
        assert!(cache
            .previous(&ReadVersion::capture(&dir, &stale_args).unwrap())
            .is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pruning_preserves_failure_status_recent_results_and_full_evidence() {
        let dir = temp_dir();
        let original = "古い出力".repeat(2000);
        let failure = format!("{}\n[exit: 1]", "failure".repeat(2000));
        let tool_error = format!("error: {}", "diagnostic".repeat(2000));
        let mut messages = vec![
            Message::user("keep this request"),
            Message::tool("old", &original),
            Message::tool("failed", &failure),
            Message::tool("tool-error", &tool_error),
            Message::tool("recent", &original),
            Message::assistant("decision", vec![]),
        ];
        let config = ToolHistory {
            prune_above_tokens: 1,
            keep_recent: 1,
            ..Default::default()
        };
        let mut saved = Vec::new();
        let reclaimed = prune_tool_history(&mut messages, usize::MAX, &config, |text| {
            let path = dir.join(format!("full-{}.txt", saved.len()));
            std::fs::write(&path, text).unwrap();
            saved.push(path.clone());
            Some(path)
        });
        assert!(reclaimed > 0);
        assert_eq!(messages[1].tool_call_id.as_deref(), Some("old"));
        assert!(messages[1].display().unwrap().contains("full-0.txt"));
        assert_eq!(std::fs::read_to_string(&saved[0]).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&saved[1]).unwrap(), failure);
        assert_eq!(std::fs::read_to_string(&saved[2]).unwrap(), tool_error);
        for index in [2, 3] {
            let excerpt = messages[index].display().unwrap();
            assert!(excerpt.contains("original tool failed;"));
            assert!(excerpt.len() < config.max_result_bytes + 300);
        }
        assert!(messages[2].display().unwrap().ends_with("[exit: 1]"));
        assert!(messages[3].display().unwrap().contains("\nerror: "));
        assert_eq!(messages[4].display().unwrap(), original);
        assert_eq!(messages[0].display().unwrap(), "keep this request");
        assert_eq!(messages[5].display().unwrap(), "decision");
        assert_eq!(
            prune_tool_history(&mut messages, usize::MAX, &config, |_| panic!(
                "already pruned"
            )),
            0
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pruning_unmeasured_results_does_not_subtract_from_the_previous_request() {
        let mut messages = vec![
            Message::tool("new", "x".repeat(10_000)),
            Message::tool("recent", "ok"),
        ];
        let config = ToolHistory {
            prune_above_tokens: 1,
            keep_recent: 1,
            ..Default::default()
        };
        let measured_reclaimed =
            prune_tool_history(&mut messages, 0, &config, |_| Some("/tmp/evidence".into()));
        assert_eq!(measured_reclaimed, 0);
        assert!(messages[0]
            .display()
            .unwrap()
            .starts_with("[tool output compacted;"));
    }

    #[test]
    fn pruning_does_not_drop_evidence_when_saving_fails() {
        let mut messages = vec![
            Message::tool("old", "x".repeat(10_000)),
            Message::tool("recent", "ok"),
        ];
        let config = ToolHistory {
            prune_above_tokens: 1,
            keep_recent: 1,
            ..Default::default()
        };
        assert_eq!(
            prune_tool_history(&mut messages, usize::MAX, &config, |_| None),
            0
        );
        assert_eq!(messages[0].display().unwrap().len(), 10_000);
    }

    #[test]
    fn image_history_batches_reclamation_and_allows_reinspection() {
        let image = |id: usize| ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: format!("data:image/png;base64,{}-{id}", "a".repeat(10_000)),
                detail: None,
            },
        };
        let mut messages = vec![Message::tool_parts(
            "one",
            "first screenshot",
            vec![image(1)],
        )];
        let user = Message {
            role: "user".into(),
            ..Message::tool_parts("user", "reference", vec![image(0)])
        };
        messages.insert(0, user.clone());
        let config = ToolHistory {
            keep_recent_images: 1,
            ..Default::default()
        };
        messages.push(Message::tool_parts("two", "second", vec![image(2)]));
        assert_eq!(
            prune_tool_images(&mut messages, usize::MAX, &config, |_| panic!("not yet")),
            0
        );
        let latest = Message::tool_parts("three", "latest", vec![image(3)]);
        messages.push(latest.clone());
        let before = serde_json::to_vec(&messages).unwrap().len();
        let mut saved = Vec::new();
        assert_eq!(
            prune_tool_images(&mut messages, usize::MAX, &config, |image| {
                saved.push(image.clone());
                Some(format!("/tmp/screenshot-{}.png", saved.len()).into())
            }),
            2
        );
        assert_eq!(saved.len(), 2);
        assert_eq!(messages[0].content, user.content);
        assert_eq!(messages[3].content, latest.content);
        assert_eq!(messages[1].tool_call_id.as_deref(), Some("one"));
        assert!(messages[1].display().unwrap().contains("first screenshot"));
        assert!(messages[1].display().unwrap().contains("read on this path"));
        assert!(serde_json::to_vec(&messages).unwrap().len() < before * 3 / 4);
        assert_eq!(
            prune_tool_images(&mut messages, usize::MAX, &config, |_| panic!(
                "already pruned"
            )),
            0
        );
        messages.push(Message::tool_parts("reinspect", "restored", vec![image(1)]));
        assert_eq!(
            prune_tool_images(&mut messages, usize::MAX, &config, |_| panic!(
                "keep restored"
            )),
            0
        );
    }

    #[test]
    fn image_history_preserves_unsaved_images_documents_and_disabled_history() {
        let mut messages = vec![
            Message::tool_parts(
                "mixed",
                "evidence",
                vec![
                    ContentPart::File {
                        file: crate::llm::FileData {
                            filename: Some("report.pdf".into()),
                            file_data: "data:application/pdf;base64,abc".into()
                        }
                    },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "https://example.org/image.png".into(),
                            detail: None
                        }
                    },
                ]
            );
            3
        ];
        let original = serde_json::to_vec(&messages).unwrap();
        let mut config = ToolHistory {
            keep_recent_images: 1,
            ..Default::default()
        };
        assert_eq!(
            prune_tool_images(&mut messages, 0, &config, |_| panic!("unseen images")),
            0
        );
        assert_eq!(serde_json::to_vec(&messages).unwrap(), original);
        assert_eq!(
            prune_tool_images(&mut messages, usize::MAX, &config, |_| None),
            0
        );
        assert_eq!(serde_json::to_vec(&messages).unwrap(), original);
        config.enabled = false;
        assert_eq!(
            prune_tool_images(&mut messages, usize::MAX, &config, |_| panic!("disabled")),
            0
        );
        config.enabled = true;
        config.keep_recent_images = 0;
        assert_eq!(
            prune_tool_images(&mut messages, usize::MAX, &config, |_| panic!("unlimited")),
            0
        );
    }
}
