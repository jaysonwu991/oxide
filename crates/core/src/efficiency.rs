use crate::llm::{Message, MessageContent, Usage};
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
}

impl Default for ToolHistory {
    fn default() -> Self {
        Self {
            enabled: true,
            prune_above_tokens: 32_000,
            keep_recent: 8,
            max_result_bytes: 2048,
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
        if text.len() <= config.max_result_bytes
            || text.starts_with("[tool output compacted;")
            || text.starts_with("error:")
            || failed_shell(text)
        {
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
        let shortened = format!("[tool output compacted; full: {}. Read this file for omitted evidence.]\n{}\n… omitted stale output …\n{}", path.display(), &text[..head], &text[tail..]);
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

    #[test]
    fn pruning_preserves_errors_recent_results_and_full_evidence() {
        let dir = temp_dir();
        let original = "古い出力".repeat(2000);
        let failure = format!("{}\n[exit: 1]", "failure".repeat(2000));
        let mut messages = vec![
            Message::user("keep this request"),
            Message::tool("old", &original),
            Message::tool("failed", &failure),
            Message::tool("recent", &original),
            Message::assistant("decision", vec![]),
        ];
        let config = ToolHistory {
            prune_above_tokens: 1,
            keep_recent: 1,
            ..Default::default()
        };
        let saved = dir.join("full.txt");
        let reclaimed = prune_tool_history(&mut messages, usize::MAX, &config, |text| {
            std::fs::write(&saved, text).unwrap();
            Some(saved.clone())
        });
        assert!(reclaimed > 0);
        assert_eq!(messages[1].tool_call_id.as_deref(), Some("old"));
        assert!(messages[1].display().unwrap().contains("full.txt"));
        assert_eq!(std::fs::read_to_string(saved).unwrap(), original);
        assert_eq!(messages[2].display().unwrap(), failure);
        assert_eq!(messages[3].display().unwrap(), original);
        assert_eq!(messages[0].display().unwrap(), "keep this request");
        assert_eq!(messages[4].display().unwrap(), "decision");
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
}
