use crate::agent::{ApprovalRequest, Steering};
use crate::config::{Mode, Reasoning};
use crate::llm::{ContentPart, Message};
use crate::plugin_registry::{MarketplaceOverview, MarketplacePluginOverview};
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

/// A running `task` subagent: what it is doing and how much it has done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentState {
    pub agent: String,
    pub tool: String,
    pub args: String,
    pub tools: usize,
}

#[derive(Debug, Clone)]
pub enum ChatItem {
    Banner {
        info: Vec<String>,
    },
    User(String),
    Assistant(String),
    /// The model's reasoning for one step. `millis` is set once the reasoning
    /// ends, which turns the streaming label into a duration.
    Thinking {
        text: String,
        millis: Option<u64>,
    },
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
    /// A titled list rendered as aligned rows with per-row status colors,
    /// instead of one plain wrapped paragraph (`/mcps`, `/plugins`).
    Listing {
        title: String,
        rows: Vec<ListRow>,
    },
    /// A one-off tip about something that just happened (`/copy`, a toggle, a
    /// theme change). Only the newest one is kept so repeated actions do not
    /// stack up lines, matching Pi's `showStatus`.
    Status(String),
    /// A background command still running, rendered as an animated progress bar
    /// until its result replaces it (`/mcps`, `/plugins list`).
    Progress {
        label: String,
        since: Instant,
    },
}

/// Semantic color for a [`ListRow`], resolved against the active theme so the
/// data model stays independent of the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Success,
    Warning,
    Error,
    Dim,
}

/// One item in a [`ChatItem::Listing`]: a name, an optional status tag, an
/// optional detail shown beside the status when it fits, and dim notes printed
/// underneath. Details and notes wrap with a hanging indent so a long URL or
/// path stays attached to its row.
#[derive(Debug, Clone)]
pub struct ListRow {
    pub name: String,
    pub status: Option<String>,
    pub tone: Tone,
    pub detail: Option<String>,
    pub notes: Vec<String>,
}

impl ListRow {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: None,
            tone: Tone::Plain,
            detail: None,
            notes: Vec::new(),
        }
    }

    pub fn status(mut self, status: impl Into<String>, tone: Tone) -> Self {
        self.status = Some(status.into());
        self.tone = tone;
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
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
    /// Providers that already have a stored credential, so the dialog can mark
    /// them and switch to one without asking for the key again.
    pub connected: Vec<String>,
}

impl ConnectState {
    pub fn new() -> Self {
        Self {
            step: ConnectStep::Provider,
            input: String::new(),
            selected: 0,
            error: None,
            connected: crate::auth::stored_providers(),
        }
    }

    /// Whether a provider already has a credential to reuse.
    pub fn is_connected(&self, provider: &str) -> bool {
        let provider = crate::auth::canonical_provider(provider);
        self.connected.iter().any(|name| name == &provider)
    }
}

impl Default for ConnectState {
    fn default() -> Self {
        Self::new()
    }
}

/// One editable row of the `/usage` dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageField {
    Enabled,
    User,
    Metadata,
    Budget,
    Currency,
    ApiKey,
    Endpoint,
}

impl UsageField {
    pub const ALL: [UsageField; 7] = [
        UsageField::Enabled,
        UsageField::User,
        UsageField::Metadata,
        UsageField::Budget,
        UsageField::Currency,
        UsageField::ApiKey,
        UsageField::Endpoint,
    ];

    pub fn label(self) -> &'static str {
        match self {
            UsageField::Enabled => "Enabled",
            UsageField::User => "User",
            UsageField::Metadata => "Metadata",
            UsageField::Budget => "Budget",
            UsageField::Currency => "Currency",
            UsageField::ApiKey => "API key",
            UsageField::Endpoint => "Endpoint",
        }
    }

    /// Whether Enter flips the value instead of opening the text editor.
    pub fn is_toggle(self) -> bool {
        matches!(self, UsageField::Enabled | UsageField::Currency)
    }
}

/// The `/usage` dialog: a form over a working copy of the Portkey spend-bar
/// settings that is committed when the dialog closes.
#[derive(Debug, Clone)]
pub struct UsageState {
    pub settings: crate::portkey_usage::UsageSettings,
    pub selected: usize,
    pub editing: bool,
    pub input: String,
    pub error: Option<String>,
}

impl UsageState {
    pub fn new(settings: crate::portkey_usage::UsageSettings) -> Self {
        Self {
            settings,
            selected: 0,
            editing: false,
            input: String::new(),
            error: None,
        }
    }

    pub fn field(&self) -> UsageField {
        UsageField::ALL[self.selected.min(UsageField::ALL.len() - 1)]
    }
}

/// One selectable model, tagged with the provider that exposes it. Several
/// logged-in providers can offer the same model id, so the pair is the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub provider: String,
    pub model: String,
}

impl ModelChoice {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModelsState {
    pub loading: bool,
    pub all: Vec<ModelChoice>,
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
        models: Vec<ModelChoice>,
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
    pub fn filtered(&self) -> Vec<&ModelChoice> {
        let filter = self.filter.to_ascii_lowercase();
        let mut models: Vec<&ModelChoice> = self
            .all
            .iter()
            .filter(|choice| {
                filter.is_empty()
                    || choice.model.to_ascii_lowercase().contains(&filter)
                    || crate::config::model_label(&choice.model)
                        .to_ascii_lowercase()
                        .contains(&filter)
            })
            .collect();
        let current = self.current.as_str();
        let provider = self.provider.as_str();
        let default = self.default.as_deref();
        models.sort_by_key(|choice| {
            if choice.model == current && choice.provider == provider {
                0
            } else if Some(choice.model.as_str()) == default {
                1
            } else {
                2
            }
        });
        models
    }

    pub fn selected_model(&self) -> Option<&ModelChoice> {
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

/// Which list the `/marketplaces` overlay is driving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarketplacePane {
    #[default]
    Marketplaces,
    Plugins,
}

/// Interactive marketplace browser shown by `/marketplaces`.
#[derive(Debug, Clone, Default)]
pub struct MarketplacesState {
    pub all: Vec<MarketplaceOverview>,
    pub filter: String,
    pub selected: usize,
    pub plugin_selected: usize,
    pub pane: MarketplacePane,
    pub adding: bool,
    pub add_input: String,
    pub confirm_remove: bool,
    pub busy: bool,
    pub message: Option<String>,
    pub error: Option<String>,
}

impl MarketplacesState {
    pub fn ready(all: Vec<MarketplaceOverview>) -> Self {
        Self {
            all,
            ..Self::default()
        }
    }

    /// Marketplaces matching the filter while that pane is active.
    pub fn marketplaces(&self) -> Vec<&MarketplaceOverview> {
        let filter = if self.pane == MarketplacePane::Marketplaces {
            self.filter.to_ascii_lowercase()
        } else {
            String::new()
        };
        self.all
            .iter()
            .filter(|marketplace| {
                filter.is_empty()
                    || marketplace.name.to_ascii_lowercase().contains(&filter)
                    || marketplace.source.to_ascii_lowercase().contains(&filter)
            })
            .collect()
    }

    pub fn selected_marketplace(&self) -> Option<&MarketplaceOverview> {
        self.marketplaces().get(self.selected).copied()
    }

    /// Plugins of the selected marketplace matching the filter while that pane
    /// is active. A name match wins outright, so a query like `doc` stays on
    /// `doc-mcp` instead of also surfacing every plugin whose description
    /// happens to mention "documentation"; descriptions are only searched as a
    /// fallback when no name matches.
    pub fn plugins(&self) -> Vec<&MarketplacePluginOverview> {
        let Some(marketplace) = self.selected_marketplace() else {
            return Vec::new();
        };
        let filter = if self.pane == MarketplacePane::Plugins {
            self.filter.to_ascii_lowercase()
        } else {
            String::new()
        };
        if filter.is_empty() {
            return marketplace.plugins.iter().collect();
        }
        let by_name: Vec<&MarketplacePluginOverview> = marketplace
            .plugins
            .iter()
            .filter(|plugin| plugin.name.to_ascii_lowercase().contains(&filter))
            .collect();
        if !by_name.is_empty() {
            return by_name;
        }
        marketplace
            .plugins
            .iter()
            .filter(|plugin| {
                plugin
                    .description
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains(&filter)
            })
            .collect()
    }

    pub fn selected_plugin(&self) -> Option<MarketplacePluginOverview> {
        self.plugins().get(self.plugin_selected).copied().cloned()
    }

    /// Number of distinct installed plugins across every marketplace. Plugins
    /// are keyed globally by name, so a name listed by several marketplaces is
    /// counted once.
    pub fn installed_count(&self) -> usize {
        self.all
            .iter()
            .flat_map(|marketplace| &marketplace.plugins)
            .filter(|plugin| plugin.installed)
            .map(|plugin| plugin.name.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }

    /// Clamps the selection indices after a reload or a filter change.
    pub fn clamp_selection(&mut self) {
        let marketplaces = self.marketplaces().len();
        self.selected = self.selected.min(marketplaces.saturating_sub(1));
        let plugins = self.plugins().len();
        self.plugin_selected = self.plugin_selected.min(plugins.saturating_sub(1));
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
    pub marketplaces: Option<MarketplacesState>,
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
    /// The `/usage` settings dialog, open while some field is being edited.
    pub usage_modal: Option<UsageState>,
    pub session_name: Option<String>,
    pub theme: crate::theme::Theme,
    pub steering: Steering,
    pub follow_ups: Steering,
    pub expand_tools: bool,
    /// Whether reasoning blocks are shown in full or collapsed to a label.
    pub show_thinking_blocks: bool,
    pub lines: Vec<Line<'static>>,
    pub line_offsets: Vec<usize>,
    pub render_dirty_from: Option<usize>,
    pub render_width: usize,
    pub selection: Option<Selection>,
    /// Name and start time of the tool currently running, for a live `Elapsed`.
    pub running_tool: Option<(String, Instant)>,
    /// The subagent a running `task` call spawned, if any.
    pub subagent: Option<SubagentState>,
    /// Whether the in-flight run is an agent turn whose completion should raise
    /// a toast. Internal runs (`/compact`, branch summaries) leave it false.
    pub notify_on_finish: bool,
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
            marketplaces: None,
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
            usage_modal: None,
            session_name: None,
            theme: crate::theme::Theme::default(),
            steering: Steering::new(),
            follow_ups: Steering::new(),
            expand_tools: false,
            show_thinking_blocks: true,
            lines: Vec::new(),
            line_offsets: Vec::new(),
            render_dirty_from: Some(0),
            render_width: 0,
            selection: None,
            running_tool: None,
            subagent: None,
            notify_on_finish: false,
        }
    }

    /// A short plain-text summary of the newest assistant reply, used as the
    /// body of the completion toast.
    pub fn completion_summary(&self) -> String {
        let Some(message) = self.history.iter().rev().find(|m| m.role == "assistant") else {
            return "Turn complete".to_string();
        };
        let Some(text) = message.display() else {
            return "Turn complete".to_string();
        };
        let summary = bounded_summary(&text, COMPLETION_SUMMARY_LIMIT);
        if summary.is_empty() {
            "Turn complete".to_string()
        } else {
            summary
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

    /// Toggle whether reasoning blocks are expanded or collapsed to a label,
    /// like Pi's `app.thinking.toggle`.
    pub fn toggle_thinking_blocks(&mut self) {
        self.show_thinking_blocks = !self.show_thinking_blocks;
        self.lines.clear();
        self.line_offsets.clear();
        self.render_dirty_from = Some(0);
    }

    /// Appends a streamed reasoning fragment to the block in progress, opening
    /// a new one when the previous block already has a duration.
    pub fn push_thinking_delta(&mut self, delta: String) {
        if !matches!(
            self.items.last(),
            Some(ChatItem::Thinking { millis: None, .. })
        ) {
            self.items.push(ChatItem::Thinking {
                text: String::new(),
                millis: None,
            });
        }
        let index = self.items.len() - 1;
        if let Some(ChatItem::Thinking { text, .. }) = self.items.last_mut() {
            text.push_str(&delta);
        }
        self.mark_render_dirty(index);
    }

    /// Closes the reasoning block in progress with the time it took.
    pub fn finish_thinking(&mut self, millis: u64) {
        let Some(index) = self.items.len().checked_sub(1) else {
            return;
        };
        if let Some(ChatItem::Thinking { millis: taken, .. }) = self.items.last_mut() {
            if taken.is_none() {
                *taken = Some(millis);
                self.mark_render_dirty(index);
            }
        }
    }

    /// Whether a reasoning block is currently streaming.
    pub fn thinking_open(&self) -> bool {
        matches!(
            self.items.last(),
            Some(ChatItem::Thinking { millis: None, .. })
        )
    }

    /// Drops a reasoning block that is still streaming. A failed attempt is
    /// retried from scratch, so its partial reasoning is discarded rather than
    /// left to be extended by the new attempt.
    pub fn discard_thinking(&mut self) {
        if self.thinking_open() {
            self.items.pop();
            self.mark_render_dirty(self.items.len());
        }
    }

    pub fn mark_render_dirty(&mut self, index: usize) {
        self.render_dirty_from = Some(
            self.render_dirty_from
                .map(|current| current.min(index))
                .unwrap_or(index),
        );
    }

    /// Shows a one-off tip in the transcript, so feedback for an action taken
    /// while idle (a copy, a toggle) is actually visible — the busy-phase
    /// `status` only renders in the composer rule. Replaces the previous tip
    /// when it is still the newest item, like Pi's `showStatus`.
    pub fn show_status(&mut self, message: impl Into<String>) {
        let message = message.into();
        if let Some(ChatItem::Status(previous)) = self.items.last_mut() {
            *previous = message;
            let index = self.items.len() - 1;
            self.mark_render_dirty(index);
            return;
        }
        self.items.push(ChatItem::Status(message));
        self.auto_scroll = true;
    }

    /// Shows an animated progress bar for a background command, replacing any
    /// previous one. The transcript redraws on the tick while it is present.
    pub fn show_progress(&mut self, label: impl Into<String>) {
        self.clear_progress();
        self.items.push(ChatItem::Progress {
            label: label.into(),
            since: Instant::now(),
        });
        self.auto_scroll = true;
    }

    /// Removes the progress bar once the command's result is available.
    pub fn clear_progress(&mut self) {
        if let Some(index) = self
            .items
            .iter()
            .position(|item| matches!(item, ChatItem::Progress { .. }))
        {
            self.items.remove(index);
            self.mark_render_dirty(index);
        }
    }

    /// Whether a background command is still running.
    pub fn has_progress(&self) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item, ChatItem::Progress { .. }))
    }

    /// Marks the progress bar dirty so it advances on each tick.
    pub fn mark_progress_dirty(&mut self) {
        if let Some(index) = self
            .items
            .iter()
            .rposition(|item| matches!(item, ChatItem::Progress { .. }))
        {
            self.mark_render_dirty(index);
        }
    }

    /// Messages queued for the running turn, steering plus follow-ups.
    pub fn queued_count(&self) -> usize {
        self.steering.len() + self.follow_ups.len()
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

/// The maximum length of the completion toast body, including the ellipsis.
const COMPLETION_SUMMARY_LIMIT: usize = 160;

/// Collapses whitespace and truncates to `max` characters, stopping as soon as
/// the limit is reached so a very long final reply is not normalized in full on
/// the event-loop thread.
fn bounded_summary(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let keep = max - 1;
    let mut out = String::new();
    let mut used = 0usize;
    let mut truncated = false;
    for word in text.split_whitespace() {
        let separator = usize::from(!out.is_empty());
        let length = word.chars().count();
        if used + separator + length > keep {
            if separator == 1 && used < keep {
                out.push(' ');
                used += 1;
            }
            let take = keep.saturating_sub(used);
            out.extend(word.chars().take(take));
            truncated = true;
            break;
        }
        if separator == 1 {
            out.push(' ');
            used += 1;
        }
        out.push_str(word);
        used += length;
    }
    if truncated {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_output_is_previewed_by_default() {
        let app = App::new("gpt-4o".into(), ".".into(), Mode::Build, Reasoning::Auto);
        assert!(!app.expand_tools);
        assert!(app.show_thinking_blocks);
    }

    fn test_app() -> App {
        App::new("gpt-4o".into(), ".".into(), Mode::Build, Reasoning::Auto)
    }

    #[test]
    fn completion_summary_uses_the_last_assistant_reply() {
        let mut app = test_app();
        app.history = vec![
            Message::user("do the thing"),
            Message::assistant("first", vec![]),
            Message::assistant("all  done\n\non two lines", vec![]),
        ];
        assert_eq!(app.completion_summary(), "all done on two lines");
    }

    #[test]
    fn completion_summary_ignores_older_replies_after_a_tool_call_only_turn() {
        let mut app = test_app();
        app.history = vec![
            Message::assistant("the earlier answer", vec![]),
            Message::assistant("", vec![]),
        ];
        assert_eq!(app.completion_summary(), "Turn complete");
    }

    #[test]
    fn completion_summary_truncates_and_falls_back() {
        let mut app = test_app();
        assert_eq!(app.completion_summary(), "Turn complete");
        app.history = vec![Message::assistant("x".repeat(400), vec![])];
        let summary = app.completion_summary();
        assert_eq!(summary.chars().count(), 160);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn progress_replaces_itself_and_clears() {
        let mut app = test_app();
        app.show_progress("Loading plugins");
        assert!(app.has_progress());
        assert_eq!(app.items.len(), 1);

        app.show_progress("Checking MCP servers");
        assert_eq!(app.items.len(), 1);
        if let ChatItem::Progress { label, .. } = &app.items[0] {
            assert_eq!(label, "Checking MCP servers");
        } else {
            panic!("expected a progress item");
        }

        app.clear_progress();
        assert!(!app.has_progress());
        assert!(app.items.is_empty());
    }

    #[test]
    fn thinking_deltas_accumulate_into_one_block() {
        let mut app = test_app();
        app.push_thinking_delta("weighing ".into());
        app.push_thinking_delta("options".into());
        assert!(app.thinking_open());
        assert_eq!(app.items.len(), 1);
        match &app.items[0] {
            ChatItem::Thinking { text, millis } => {
                assert_eq!(text, "weighing options");
                assert_eq!(*millis, None);
            }
            other => panic!("unexpected item: {other:?}"),
        }
        app.finish_thinking(1200);
        assert!(!app.thinking_open());
        assert!(matches!(
            app.items[0],
            ChatItem::Thinking {
                millis: Some(1200),
                ..
            }
        ));
    }

    #[test]
    fn a_finished_block_is_not_extended_by_later_fragments() {
        let mut app = test_app();
        app.push_thinking_delta("first".into());
        app.finish_thinking(400);
        app.push_thinking_delta("second".into());
        assert_eq!(app.items.len(), 2);
        assert!(app.thinking_open());
    }

    #[test]
    fn discarding_thinking_drops_only_a_streaming_block() {
        let mut app = test_app();
        app.items.push(ChatItem::User("hi".into()));
        app.push_thinking_delta("partial".into());
        app.discard_thinking();
        assert_eq!(app.items.len(), 1);
        assert!(!app.thinking_open());

        app.push_thinking_delta("kept".into());
        app.finish_thinking(300);
        app.discard_thinking();
        assert_eq!(app.items.len(), 2);
        assert!(matches!(app.items[1], ChatItem::Thinking { .. }));
    }

    #[test]
    fn toggling_thinking_blocks_rerenders_the_transcript() {
        let mut app = test_app();
        app.lines = lines(&["stale"]);
        app.render_dirty_from = None;
        app.toggle_thinking_blocks();
        assert!(!app.show_thinking_blocks);
        assert!(app.lines.is_empty());
        assert_eq!(app.render_dirty_from, Some(0));
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

    fn marketplace(name: &str, plugins: &[(&str, bool)]) -> MarketplaceOverview {
        MarketplaceOverview {
            name: name.into(),
            source: format!("https://example.com/{name}"),
            path: format!("/tmp/{name}").into(),
            owner: None,
            plugins: plugins
                .iter()
                .map(|(plugin, installed)| MarketplacePluginOverview {
                    name: (*plugin).into(),
                    description: None,
                    version: None,
                    installed: *installed,
                    enabled: *installed,
                })
                .collect(),
            error: None,
        }
    }

    #[test]
    fn marketplace_filter_applies_to_the_active_pane_only() {
        let mut state = MarketplacesState::ready(vec![
            marketplace("alpha", &[("one", true), ("two", false)]),
            marketplace("beta", &[("three", false)]),
        ]);

        state.filter = "alp".into();
        assert_eq!(state.marketplaces().len(), 1);
        assert_eq!(state.plugins().len(), 2);

        state.pane = MarketplacePane::Plugins;
        state.filter = "tw".into();
        assert_eq!(state.marketplaces().len(), 2);
        assert_eq!(state.plugins().len(), 1);
        assert_eq!(state.selected_plugin().unwrap().name, "two");
        assert_eq!(state.installed_count(), 1);
    }

    #[test]
    fn marketplace_filter_prefers_plugin_names_over_descriptions() {
        let mut state = MarketplacesState::ready(vec![MarketplaceOverview {
            name: "shop".into(),
            source: "https://example.com/shop".into(),
            path: "/tmp/shop".into(),
            owner: None,
            plugins: vec![
                MarketplacePluginOverview {
                    name: "doc-mcp".into(),
                    description: Some("Retrieve documentation".into()),
                    version: None,
                    installed: false,
                    enabled: false,
                },
                MarketplacePluginOverview {
                    name: "content-review".into(),
                    description: Some("Review written documentation".into()),
                    version: None,
                    installed: false,
                    enabled: false,
                },
                MarketplacePluginOverview {
                    name: "onboarding".into(),
                    description: Some("Generate onboarding guides".into()),
                    version: None,
                    installed: false,
                    enabled: false,
                },
            ],
            error: None,
        }]);
        state.pane = MarketplacePane::Plugins;

        state.filter = "doc".into();
        let names: Vec<&str> = state.plugins().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["doc-mcp"]);

        // With no name match, the description is still searched.
        state.filter = "written".into();
        let names: Vec<&str> = state.plugins().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["content-review"]);
    }

    #[test]
    fn marketplace_selection_clamps_after_filtering() {
        let mut state = MarketplacesState::ready(vec![
            marketplace("alpha", &[("one", true)]),
            marketplace("beta", &[]),
        ]);
        state.selected = 1;
        state.filter = "alpha".into();
        state.clamp_selection();
        assert_eq!(state.selected, 0);
        assert_eq!(state.selected_marketplace().unwrap().name, "alpha");
    }
}
