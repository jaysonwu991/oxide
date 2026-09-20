use crate::agent::{ApprovalRequest, Steering};
use crate::config::{Mode, Reasoning};
use crate::llm::{ContentPart, Message};
use crate::session::SessionSummary;
use crate::tools::DiffPreview;
use ratatui::text::Line;
use std::process::Command;
use std::time::Instant;

/// A mouse text selection over the rendered conversation, in absolute line and
/// column coordinates. Display columns index characters because the renderer
/// wraps by character width.
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    pub anchor: (usize, usize),
    pub cursor: (usize, usize),
}

impl Selection {
    pub fn new(line: usize, column: usize) -> Self {
        Self {
            anchor: (line, column),
            cursor: (line, column),
        }
    }

    /// The selection endpoints ordered so the start precedes the end. Both are
    /// inclusive.
    pub fn range(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// Whether the cell at `line`/`column` falls inside the selection.
    pub fn contains(&self, line: usize, column: usize) -> bool {
        let (start, end) = self.range();
        let position = (line, column);
        position >= start && position <= end
    }

    /// The selected text extracted from the rendered lines, with trailing
    /// whitespace trimmed from each line.
    pub fn text(&self, lines: &[Line<'static>]) -> String {
        if lines.is_empty() {
            return String::new();
        }
        let last = lines.len() - 1;
        let (start, end) = self.range();
        let stop = end.0.min(last);
        let mut out: Vec<String> = Vec::new();
        for (index, line) in lines.iter().enumerate().take(stop + 1).skip(start.0) {
            let chars: Vec<char> = line
                .spans
                .iter()
                .flat_map(|span| span.content.chars())
                .collect();
            let from = if index == start.0 { start.1 } else { 0 }.min(chars.len());
            let to = if index == end.0 {
                end.1.saturating_add(1)
            } else {
                chars.len()
            }
            .clamp(from, chars.len());
            let text: String = chars[from..to].iter().collect();
            out.push(text.trim_end().to_string());
        }
        out.join("\n")
    }
}

#[derive(Debug, Clone)]
pub enum ChatItem {
    Banner {
        info: Vec<String>,
    },
    User(String),
    Assistant(String),
    Tool {
        name: String,
        args: String,
    },
    ToolProgress {
        name: String,
        output: String,
    },
    ToolResult {
        name: String,
        args: String,
        output: String,
        diff: Option<DiffPreview>,
        millis: u64,
    },
    Compaction {
        summary: String,
        summarized: usize,
        tokens_before: u64,
        read_files: Vec<String>,
        modified_files: Vec<String>,
    },
    Error(String),
    Info(String),
}

#[derive(Debug, Clone)]
pub enum ConnectStep {
    Provider,
    Key { provider: String },
}

#[derive(Debug, Clone)]
pub struct ConnectState {
    pub step: ConnectStep,
    pub input: String,
    pub selected: usize,
    pub error: Option<String>,
}

impl ConnectState {
    pub fn new() -> Self {
        Self {
            step: ConnectStep::Provider,
            input: String::new(),
            selected: 0,
            error: None,
        }
    }
}

impl Default for ConnectState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModelsState {
    pub loading: bool,
    pub all: Vec<String>,
    pub filter: String,
    pub selected: usize,
    pub error: Option<String>,
    pub provider: String,
    pub current: String,
    pub default: Option<String>,
    pub refreshed: bool,
}

impl ModelsState {
    pub fn loading(provider: String, current: String, default: Option<String>) -> Self {
        Self {
            loading: true,
            provider,
            current,
            default,
            ..Self::default()
        }
    }

    pub fn ready(
        models: Vec<String>,
        provider: String,
        current: String,
        default: Option<String>,
    ) -> Self {
        Self {
            all: models,
            provider,
            current,
            default,
            refreshed: true,
            ..Self::default()
        }
    }

    /// Models matching the current filter, ordered current first, then the
    /// persisted default, then the provider's natural order.
    pub fn filtered(&self) -> Vec<&str> {
        let filter = self.filter.to_ascii_lowercase();
        let mut models: Vec<&str> = self
            .all
            .iter()
            .map(String::as_str)
            .filter(|model| {
                filter.is_empty()
                    || model.to_ascii_lowercase().contains(&filter)
                    || crate::config::model_label(model)
                        .to_ascii_lowercase()
                        .contains(&filter)
            })
            .collect();
        let current = self.current.as_str();
        let default = self.default.as_deref();
        models.sort_by_key(|model| {
            if *model == current {
                0
            } else if Some(*model) == default {
                1
            } else {
                2
            }
        });
        models
    }

    pub fn selected_model(&self) -> Option<&str> {
        self.filtered().get(self.selected).copied()
    }
}

/// A slash-command entry shown in the input autocomplete.
#[derive(Debug, Clone)]
pub struct CommandHint {
    pub name: String,
    pub description: String,
}

/// Interactive session picker shown by `/resume` and `oxide -r`.
#[derive(Debug, Clone, Default)]
pub struct SessionsState {
    pub all: Vec<SessionSummary>,
    pub filter: String,
    pub selected: usize,
    pub show_paths: bool,
    pub newest_first: bool,
    pub named_only: bool,
    pub confirm_delete: bool,
    pub renaming: bool,
    pub rename_input: String,
    pub error: Option<String>,
}

impl SessionsState {
    pub fn ready(sessions: Vec<SessionSummary>) -> Self {
        Self {
            all: sessions,
            newest_first: true,
            ..Self::default()
        }
    }

    /// Sessions matching the current filter and sort, in display order.
    pub fn filtered(&self) -> Vec<SessionSummary> {
        let filter = self.filter.to_ascii_lowercase();
        let mut sessions: Vec<SessionSummary> = self
            .all
            .iter()
            .filter(|session| {
                (!self.named_only || session.name.is_some())
                    && (filter.is_empty()
                        || session
                            .name
                            .as_deref()
                            .unwrap_or_default()
                            .to_ascii_lowercase()
                            .contains(&filter)
                        || session.id.to_ascii_lowercase().starts_with(&filter)
                        || session.cwd.to_ascii_lowercase().contains(&filter))
            })
            .cloned()
            .collect();
        sessions.sort_by(|a, b| {
            if self.newest_first {
                b.modified_at.cmp(&a.modified_at)
            } else {
                a.modified_at.cmp(&b.modified_at)
            }
        });
        sessions
    }

    pub fn selected_session(&self) -> Option<SessionSummary> {
        self.filtered().get(self.selected).cloned()
    }
}

#[derive(Debug, Clone)]
pub struct TrustState {
    pub dir: String,
    pub resources: Vec<String>,
    pub selected: usize,
}

impl TrustState {
    pub fn new(dir: String, resources: Vec<String>) -> Self {
        Self {
            dir,
            resources,
            selected: 0,
        }
    }
}

pub struct App {
    pub input: String,
    pub input_cursor: usize,
    pub input_history: Vec<String>,
    pub history_index: Option<usize>,
    pub items: Vec<ChatItem>,
    pub history: Vec<Message>,
    pub attachments: Vec<ContentPart>,
    pub scroll: u16,
    pub auto_scroll: bool,
    pub view_height: u16,
    pub busy: bool,
    pub busy_since: Option<Instant>,
    pub status: String,
    pub should_quit: bool,
    pub model: String,
    pub provider: String,
    pub available_providers: usize,
    pub auto_compact: bool,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub assistant_open: bool,
    pub pending_approval: Option<ApprovalRequest>,
    pub connect: Option<ConnectState>,
    pub models: Option<ModelsState>,
    pub sessions: Option<SessionsState>,
    pub trust: Option<TrustState>,
    pub suggestions: Vec<CommandHint>,
    pub suggestion_index: usize,
    pub workspace_paths: Option<Vec<String>>,
    pub mode: Mode,
    pub reasoning: Reasoning,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tokens_cache_read: u64,
    pub tokens_cache_write: u64,
    pub cost: f64,
    pub cache_hit_rate: Option<f64>,
    pub context_used: u64,
    pub context_limit: u64,
    pub show_thinking: bool,
    pub extension_statuses: std::collections::BTreeMap<String, String>,
    /// Settings for the Portkey spend bar, and the bar itself when enabled.
    pub usage_settings: crate::portkey_usage::UsageSettings,
    pub usage: Option<crate::portkey_usage::UsageBar>,
    pub session_name: Option<String>,
    pub theme: crate::theme::Theme,
    pub steering: Steering,
    pub follow_ups: Steering,
    pub expand_tools: bool,
    pub lines: Vec<Line<'static>>,
    pub line_offsets: Vec<usize>,
    pub render_dirty_from: Option<usize>,
    pub render_width: usize,
    pub selection: Option<Selection>,
    /// Name and start time of the tool currently running, for a live `Elapsed`.
    pub running_tool: Option<(String, Instant)>,
}

impl App {
    pub fn refresh_git_branch(&mut self) {
        self.git_branch = current_git_branch(&self.cwd);
    }

    pub fn new(model: String, cwd: String, mode: Mode, reasoning: Reasoning) -> Self {
        let git_branch = current_git_branch(&cwd);
        Self {
            input: String::new(),
            input_cursor: 0,
            input_history: Vec::new(),
            history_index: None,
            items: Vec::new(),
            history: Vec::new(),
            attachments: Vec::new(),
            scroll: 0,
            auto_scroll: true,
            view_height: 0,
            busy: false,
            busy_since: None,
            status: "ready".to_string(),
            should_quit: false,
            model,
            provider: String::new(),
            available_providers: 0,
            auto_compact: true,
            cwd,
            git_branch,
            assistant_open: false,
            pending_approval: None,
            connect: None,
            models: None,
            sessions: None,
            trust: None,
            suggestions: Vec::new(),
            suggestion_index: 0,
            workspace_paths: None,
            mode,
            reasoning,
            tokens_in: 0,
            tokens_out: 0,
            tokens_cache_read: 0,
            tokens_cache_write: 0,
            cost: 0.0,
            cache_hit_rate: None,
            context_used: 0,
            context_limit: 0,
            show_thinking: true,
            extension_statuses: std::collections::BTreeMap::new(),
            usage_settings: crate::portkey_usage::UsageSettings::default(),
            usage: None,
            session_name: None,
            theme: crate::theme::Theme::default(),
            steering: Steering::new(),
            follow_ups: Steering::new(),
            expand_tools: true,
            lines: Vec::new(),
            line_offsets: Vec::new(),
            render_dirty_from: Some(0),
            render_width: 0,
            selection: None,
            running_tool: None,
        }
    }

    /// Replaces the conversation with new context (used after in-file branch
    /// navigation) and rebuilds the transcript from the messages.
    pub fn reset_history(&mut self, messages: Vec<Message>) {
        self.history = messages;
        let banner = self
            .items
            .first()
            .filter(|item| matches!(item, ChatItem::Banner { .. }))
            .cloned();
        self.items.clear();
        if let Some(banner) = banner {
            self.items.push(banner);
        }
        for message in &self.history {
            match message.role.as_str() {
                "user" => {
                    if let Some(text) = message.display() {
                        self.items.push(ChatItem::User(text));
                    }
                }
                "assistant" => {
                    if let Some(text) = message.display() {
                        if !text.trim().is_empty() {
                            self.items.push(ChatItem::Assistant(text));
                        }
                    }
                }
                "tool" => self.items.push(ChatItem::ToolResult {
                    name: "tool".to_string(),
                    args: "{}".to_string(),
                    output: message.display().unwrap_or_default(),
                    diff: None,
                    millis: 0,
                }),
                _ => {}
            }
        }
        self.invalidate_render_cache();
    }

    /// Rebuild styled conversation lines after a visual setting changes.
    pub fn invalidate_render_cache(&mut self) {
        self.lines.clear();
        self.line_offsets.clear();
        self.render_dirty_from = Some(0);
        self.render_width = 0;
    }

    /// Records a submitted input so it can be recalled with the Up key.
    pub fn remember_input(&mut self, raw: &str) {
        if raw.trim().is_empty() {
            self.history_index = None;
            return;
        }
        if self.input_history.last().map(String::as_str) != Some(raw) {
            self.input_history.push(raw.to_string());
        }
        self.history_index = None;
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
    }

    pub fn set_input(&mut self, input: String) {
        self.input = input;
        self.input_cursor = self.input.len();
    }

    pub fn insert_input(&mut self, text: &str) {
        self.input.insert_str(self.input_cursor, text);
        self.input_cursor += text.len();
    }

    pub fn input_cursor_left(&mut self) {
        if let Some((index, _)) = self.input[..self.input_cursor].char_indices().next_back() {
            self.input_cursor = index;
        }
    }

    pub fn input_cursor_right(&mut self) {
        if let Some(ch) = self.input[self.input_cursor..].chars().next() {
            self.input_cursor += ch.len_utf8();
        }
    }

    /// Moves the composer cursor to the start of the input.
    pub fn input_home(&mut self) {
        self.input_cursor = 0;
    }

    /// Moves the composer cursor to the end of the input.
    pub fn input_end(&mut self) {
        self.input_cursor = self.input.len();
    }

    pub fn input_backspace(&mut self) {
        let previous = self.input[..self.input_cursor]
            .char_indices()
            .next_back()
            .map(|(index, _)| index);
        if let Some(previous) = previous {
            self.input.drain(previous..self.input_cursor);
            self.input_cursor = previous;
        }
    }

    /// Recalls the previous input, walking backwards through history.
    pub fn history_prev(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let index = match self.history_index {
            Some(0) => 0,
            Some(index) => index - 1,
            None => self.input_history.len() - 1,
        };
        self.history_index = Some(index);
        let input = self.input_history[index].clone();
        self.set_input(input);
    }

    /// Recalls the next input, clearing the box past the newest entry.
    pub fn history_next(&mut self) {
        match self.history_index {
            Some(index) if index + 1 < self.input_history.len() => {
                self.history_index = Some(index + 1);
                let input = self.input_history[index + 1].clone();
                self.set_input(input);
            }
            Some(_) => {
                self.history_index = None;
                self.clear_input();
            }
            None => {}
        }
    }

    /// The number of lines in one page, based on the last rendered viewport.
    pub fn page_step(&self) -> u16 {
        self.view_height.max(1)
    }

    /// Scrolls the conversation up by `lines` and stops following new output.
    pub fn scroll_up(&mut self, lines: u16) {
        self.scroll = self.scroll.saturating_sub(lines);
        self.auto_scroll = false;
    }

    /// Scrolls the conversation down by `lines` and stops following new output.
    pub fn scroll_down(&mut self, lines: u16) {
        self.scroll = self.scroll.saturating_add(lines);
        self.auto_scroll = false;
    }

    /// Jumps to the oldest visible output and stops following new output.
    pub fn scroll_to_top(&mut self) {
        self.scroll = 0;
        self.auto_scroll = false;
    }

    /// Jumps back to the newest output and resumes following it.
    pub fn scroll_to_bottom(&mut self) {
        self.auto_scroll = true;
    }

    pub fn push_assistant_delta(&mut self, delta: String) {
        if !self.assistant_open {
            self.items.push(ChatItem::Assistant(String::new()));
            self.assistant_open = true;
        }
        let index = self.items.len() - 1;
        if let Some(ChatItem::Assistant(buffer)) = self.items.last_mut() {
            buffer.push_str(&delta);
        }
        self.mark_render_dirty(index);
    }

    /// Toggle whether long tool output is shown in full or collapsed, and
    /// invalidate the rendered-line cache so the change takes effect.
    pub fn toggle_tool_output(&mut self) {
        self.expand_tools = !self.expand_tools;
        self.lines.clear();
        self.line_offsets.clear();
        self.render_dirty_from = Some(0);
    }

    pub fn mark_render_dirty(&mut self, index: usize) {
        self.render_dirty_from = Some(
            self.render_dirty_from
                .map(|current| current.min(index))
                .unwrap_or(index),
        );
    }

    /// Marks the item rendering the running tool dirty so a live `Elapsed`
    /// updates on each tick without rebuilding the whole transcript.
    pub fn mark_running_tool_dirty(&mut self) {
        if self.running_tool.is_none() {
            return;
        }
        if let Some(index) = self
            .items
            .iter()
            .rposition(|item| matches!(item, ChatItem::Tool { .. } | ChatItem::ToolProgress { .. }))
        {
            self.mark_render_dirty(index);
        }
    }

    /// Fold a tool result into the pending call it belongs to, so the
    /// conversation shows one entry per call (like the opencode reference)
    /// rather than a call line followed by a separate result line. Falls back
    /// to appending when no matching pending call is found.
    pub fn resolve_tool(
        &mut self,
        name: String,
        args: String,
        output: String,
        diff: Option<DiffPreview>,
        millis: u64,
    ) {
        let mut progress = Vec::new();
        let mut index = self.items.len();
        let mut tool = None;
        while index > 0 {
            match &self.items[index - 1] {
                ChatItem::ToolProgress { name: pending, .. } if pending == &name => {
                    index -= 1;
                    progress.push(index);
                }
                ChatItem::Tool { name: pending, .. } if pending == &name => {
                    tool = Some(index - 1);
                    break;
                }
                _ => break,
            }
        }
        match tool {
            Some(tool) => {
                for index in progress {
                    self.items.remove(index);
                }
                self.items[tool] = ChatItem::ToolResult {
                    name,
                    args,
                    output,
                    diff,
                    millis,
                };
                self.mark_render_dirty(tool);
            }
            None => self.items.push(ChatItem::ToolResult {
                name,
                args,
                output,
                diff,
                millis,
            }),
        }
    }
}

fn current_git_branch(cwd: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", cwd, "branch", "--show-current"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?;
    let branch = branch.trim();
    (!branch.is_empty()).then(|| branch.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_output_is_expanded_by_default() {
        let app = App::new("gpt-4o".into(), ".".into(), Mode::Build, Reasoning::Auto);
        assert!(app.expand_tools);
    }

    fn lines(texts: &[&str]) -> Vec<Line<'static>> {
        texts
            .iter()
            .map(|text| Line::from(text.to_string()))
            .collect()
    }

    #[test]
    fn selection_text_extracts_inclusive_character_range() {
        let lines = lines(&["hello world", "second line"]);
        let mut selection = Selection::new(0, 6);
        selection.cursor = (1, 5);
        assert_eq!(selection.text(&lines), "world\nsecond");
    }

    #[test]
    fn selection_text_orders_reversed_drag_and_trims_trailing_space() {
        let lines = lines(&["alpha  ", "beta"]);
        let mut selection = Selection::new(1, 3);
        selection.cursor = (0, 0);
        assert_eq!(selection.text(&lines), "alpha\nbeta");
    }

    #[test]
    fn selection_contains_uses_ordered_range() {
        let mut selection = Selection::new(2, 4);
        selection.cursor = (1, 1);
        assert!(selection.contains(1, 5));
        assert!(selection.contains(2, 4));
        assert!(!selection.contains(0, 9));
        assert!(!selection.contains(3, 0));
    }
}
