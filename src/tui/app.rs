use crate::agent::{ApprovalRequest, Steering};
use crate::config::{Mode, Reasoning};
use crate::llm::{ContentPart, Message};
use crate::tools::DiffPreview;
use ratatui::text::Line;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Instant;

#[derive(Debug, Clone)]
pub enum ChatItem {
    Banner,
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
    },
    Thought(u64),
    Error(String),
    Info(String),
}

impl ChatItem {
    pub fn signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        match self {
            ChatItem::Banner => {
                7u8.hash(&mut hasher);
            }
            ChatItem::User(text) => {
                0u8.hash(&mut hasher);
                text.hash(&mut hasher);
            }
            ChatItem::Assistant(text) => {
                1u8.hash(&mut hasher);
                text.hash(&mut hasher);
            }
            ChatItem::Tool { name, args } => {
                2u8.hash(&mut hasher);
                name.hash(&mut hasher);
                args.hash(&mut hasher);
            }
            ChatItem::ToolProgress { name, output } => {
                3u8.hash(&mut hasher);
                name.hash(&mut hasher);
                output.hash(&mut hasher);
            }
            ChatItem::ToolResult {
                name,
                args,
                output,
                diff,
            } => {
                4u8.hash(&mut hasher);
                name.hash(&mut hasher);
                args.hash(&mut hasher);
                output.hash(&mut hasher);
                if let Some(diff) = diff {
                    diff.path.hash(&mut hasher);
                    diff.text.hash(&mut hasher);
                }
            }
            ChatItem::Thought(millis) => {
                7u8.hash(&mut hasher);
                millis.hash(&mut hasher);
            }
            ChatItem::Error(text) => {
                5u8.hash(&mut hasher);
                text.hash(&mut hasher);
            }
            ChatItem::Info(text) => {
                6u8.hash(&mut hasher);
                text.hash(&mut hasher);
            }
        }
        hasher.finish()
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
}

impl ModelsState {
    pub fn loading() -> Self {
        Self {
            loading: true,
            ..Self::default()
        }
    }

    pub fn ready(models: Vec<String>) -> Self {
        Self {
            all: models,
            ..Self::default()
        }
    }

    /// Models matching the current filter, in display order.
    pub fn filtered(&self) -> Vec<&str> {
        let filter = self.filter.to_ascii_lowercase();
        self.all
            .iter()
            .map(String::as_str)
            .filter(|model| filter.is_empty() || model.to_ascii_lowercase().contains(&filter))
            .collect()
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
    pub cwd: String,
    pub assistant_open: bool,
    pub pending_approval: Option<ApprovalRequest>,
    pub connect: Option<ConnectState>,
    pub models: Option<ModelsState>,
    pub trust: Option<TrustState>,
    pub suggestions: Vec<CommandHint>,
    pub suggestion_index: usize,
    pub mode: Mode,
    pub reasoning: Reasoning,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub context_used: u64,
    pub context_limit: u64,
    pub session_name: Option<String>,
    pub theme: crate::theme::Theme,
    pub steering: Steering,
    pub follow_ups: Steering,
    pub expand_tools: bool,
    pub lines: Vec<Line<'static>>,
    pub line_offsets: Vec<usize>,
    pub signatures: Vec<u64>,
    pub render_width: usize,
}

impl App {
    pub fn new(model: String, cwd: String, mode: Mode, reasoning: Reasoning) -> Self {
        Self {
            input: String::new(),
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
            cwd,
            assistant_open: false,
            pending_approval: None,
            connect: None,
            models: None,
            trust: None,
            suggestions: Vec::new(),
            suggestion_index: 0,
            mode,
            reasoning,
            tokens_in: 0,
            tokens_out: 0,
            context_used: 0,
            context_limit: 0,
            session_name: None,
            theme: crate::theme::Theme::default(),
            steering: Steering::new(),
            follow_ups: Steering::new(),
            expand_tools: false,
            lines: Vec::new(),
            line_offsets: Vec::new(),
            signatures: Vec::new(),
            render_width: 0,
        }
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
        self.input = self.input_history[index].clone();
    }

    /// Recalls the next input, clearing the box past the newest entry.
    pub fn history_next(&mut self) {
        match self.history_index {
            Some(index) if index + 1 < self.input_history.len() => {
                self.history_index = Some(index + 1);
                self.input = self.input_history[index + 1].clone();
            }
            Some(_) => {
                self.history_index = None;
                self.input.clear();
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
        if let Some(ChatItem::Assistant(buffer)) = self.items.last_mut() {
            buffer.push_str(&delta);
        }
    }

    /// Toggle whether long tool output is shown in full or collapsed, and
    /// invalidate the rendered-line cache so the change takes effect.
    pub fn toggle_tool_output(&mut self) {
        self.expand_tools = !self.expand_tools;
        self.lines.clear();
        self.line_offsets.clear();
        self.signatures.clear();
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
                };
            }
            None => self.items.push(ChatItem::ToolResult {
                name,
                args,
                output,
                diff,
            }),
        }
    }
}
