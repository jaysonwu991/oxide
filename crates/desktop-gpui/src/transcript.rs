//! One conversation as the window paints it: the items of a thread and the
//! labels they are read by.
//!
//! This module is pure — it holds no GPUI types — so the transcript's rules are
//! unit tested without a window. The view paints what it produces, and the
//! Electron front-end's own transcript is built from the same `AgentEvent`s by
//! `ui/app.js`, so a turn reads the same way in both.

use serde_json::Value;

/// A tool call as it is painted: the call, whatever it has printed so far, and
/// whether it is still running.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolItem {
    pub name: String,
    pub args: String,
    pub output: String,
    pub is_error: bool,
    pub millis: u64,
    pub running: bool,
}

impl ToolItem {
    pub fn new(name: impl Into<String>, args: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: args.into(),
            output: String::new(),
            is_error: false,
            millis: 0,
            running: true,
        }
    }

    /// The one word the card's header carries for the call.
    pub fn verb(&self) -> &'static str {
        if self.running {
            "Running"
        } else if self.is_error {
            "Failed"
        } else {
            "Ran"
        }
    }
}

/// One line of the conversation.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User(String),
    Assistant { text: String, streaming: bool },
    Thinking { text: String, millis: Option<u64> },
    Tool(ToolItem),
    Info(String),
    Error(String),
}

impl Item {
    /// Whether an item is still being written, which is what a retry drops.
    pub fn streaming(&self) -> bool {
        matches!(
            self,
            Item::Assistant {
                streaming: true,
                ..
            } | Item::Thinking { .. }
                | Item::Tool(ToolItem { running: true, .. })
        )
    }
}

/// The argument a tool call's summary is read from, in the order the tools name
/// it: `bash` takes a command, the file tools a path, the searches a pattern.
pub fn argument(args: &str, keys: &[&str]) -> Option<String> {
    let value: Value = serde_json::from_str(args).ok()?;
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

/// A one-line summary of a call: the command, the path, the pattern. Everything
/// the header needs beyond the tool's own name.
pub fn tool_summary(name: &str, args: &str) -> String {
    let summary = match name {
        "bash" => argument(args, &["command"]),
        "read" | "write" | "edit" => argument(args, &["path", "file_path"]),
        "grep" => {
            let pattern = argument(args, &["pattern", "query"]);
            let scope = argument(args, &["path"]);
            match (pattern, scope) {
                (Some(pattern), Some(scope)) => Some(format!("{pattern} in {scope}")),
                (Some(pattern), None) => Some(pattern),
                (None, _) => None,
            }
        }
        "find" | "ls" => argument(args, &["pattern", "path"]),
        "webfetch" => argument(args, &["url"]),
        "task" => argument(args, &["agent", "description", "prompt"]),
        "skill" => argument(args, &["name", "skill"]),
        "command" => argument(args, &["name", "command"]),
        "memory" => argument(args, &["query", "name"]),
        "diagnostics" => argument(args, &["path"]),
        _ => argument(args, &["path", "command", "pattern", "query", "url"]),
    };
    let summary = summary.unwrap_or_else(|| args.to_string());
    collapse(&summary)
}

/// The tool's name as it is painted: an MCP tool keeps the server it belongs to
/// and the tool's own name, the way the terminal and the Electron window name it.
pub fn tool_label(name: &str) -> String {
    match name.split_once("__") {
        Some((server, tool)) if !server.is_empty() && !tool.is_empty() => {
            format!("{server}: {tool}")
        }
        _ => name.to_string(),
    }
}

/// A preview of a tool's output: the first `max_lines` lines, with a marker
/// naming what was left. A shell's output keeps its tail, since the error a
/// command failed with is at the end, and everything else keeps its head.
pub fn preview(name: &str, output: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    if max_lines == 0 || lines.len() <= max_lines {
        return lines.join("\n");
    }
    if name == "bash" {
        let start = lines.len() - max_lines;
        format!("… {} earlier lines\n{}", start, lines[start..].join("\n"))
    } else {
        format!(
            "{}\n… {} more lines",
            lines[..max_lines].join("\n"),
            lines.len() - max_lines
        )
    }
}

/// The steps the terminal's footer abbreviates by: `999`, `1.2k`, `12k`,
/// `1.2M`, `12M`.
pub fn format_tokens(count: u64) -> String {
    if count < 1_000 {
        count.to_string()
    } else if count < 10_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else if count < 1_000_000 {
        format!("{}k", (count as f64 / 1_000.0).round() as u64)
    } else if count < 10_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else {
        format!("{}M", (count as f64 / 1_000_000.0).round() as u64)
    }
}

/// The spend line's amount, in dollars, to two places — omitted entirely by the
/// view when the model has no price in the table.
pub fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("${cost:.4}")
    } else {
        format!("${cost:.2}")
    }
}

/// A duration as the transcript reads it: `820ms`, `3.4s`, `1m 12s`.
pub fn format_duration(millis: u64) -> String {
    match millis {
        0..=999 => format!("{millis}ms"),
        1000..=59_999 => format!("{:.1}s", millis as f64 / 1000.0),
        _ => {
            let seconds = millis / 1000;
            format!("{}m {}s", seconds / 60, seconds % 60)
        }
    }
}

fn collapse(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() > 120 {
        let cut: String = joined.chars().take(119).collect();
        format!("{cut}…")
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_is_summarized_by_its_own_argument() {
        assert_eq!(
            tool_summary("bash", r#"{"command":"cargo test"}"#),
            "cargo test"
        );
        assert_eq!(
            tool_summary("read", r#"{"path":"crates/core/src/agent.rs"}"#),
            "crates/core/src/agent.rs"
        );
        assert_eq!(
            tool_summary("grep", r#"{"pattern":"pub fn","path":"crates/core"}"#),
            "pub fn in crates/core"
        );
    }

    #[test]
    fn a_summary_collapses_whitespace_and_cuts_a_long_command() {
        let long = "x".repeat(200);
        let summary = tool_summary("bash", &format!(r#"{{"command":"echo {long}"}}"#));
        assert!(!summary.contains('\n'));
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn an_unparseable_call_falls_back_to_its_arguments() {
        assert_eq!(tool_summary("bash", "not json"), "not json");
    }

    #[test]
    fn an_mcp_tool_keeps_its_server() {
        assert_eq!(tool_label("github__create_issue"), "github: create_issue");
        assert_eq!(tool_label("bash"), "bash");
    }

    #[test]
    fn a_head_preview_names_what_it_left_and_a_shell_keeps_its_tail() {
        let output = "1\n2\n3\n4";
        assert_eq!(preview("read", output, 2), "1\n2\n… 2 more lines");
        assert_eq!(preview("bash", output, 2), "… 2 earlier lines\n3\n4");
        assert_eq!(preview("read", output, 10), output);
    }

    #[test]
    fn counts_are_abbreviated_the_way_the_footer_abbreviates_them() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1234), "1.2k");
        assert_eq!(format_tokens(123_456), "123k");
        assert_eq!(format_tokens(1_234_567), "1.2M");
        assert_eq!(format_tokens(66_000_000), "66M");
    }

    #[test]
    fn a_duration_reads_in_the_unit_it_is_measured_by() {
        assert_eq!(format_duration(820), "820ms");
        assert_eq!(format_duration(3400), "3.4s");
        assert_eq!(format_duration(72_000), "1m 12s");
    }

    #[test]
    fn a_running_call_keeps_its_state_apart_from_a_finished_one() {
        let mut tool = ToolItem::new("bash", "{}");
        assert!(matches!(&Item::Tool(tool.clone()), item if item.streaming()));
        assert_eq!(tool.verb(), "Running");
        tool.running = false;
        tool.is_error = true;
        assert_eq!(tool.verb(), "Failed");
        tool.is_error = false;
        assert_eq!(tool.verb(), "Ran");
    }
}
