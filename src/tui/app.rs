use crate::agent::{ApprovalRequest, Steering};
use crate::config::{Mode, Reasoning};
use crate::llm::{ContentPart, Message};
use ratatui::text::Line;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone)]
pub enum ChatItem {
    User(String),
    Assistant(String),
    Tool { name: String, args: String },
    ToolProgress { name: String, output: String },
    ToolResult { name: String, output: String },
    Error(String),
    Info(String),
}

impl ChatItem {
    pub fn signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        match self {
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
            ChatItem::ToolResult { name, output } => {
                4u8.hash(&mut hasher);
                name.hash(&mut hasher);
                output.hash(&mut hasher);
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

pub struct App {
    pub input: String,
    pub items: Vec<ChatItem>,
    pub history: Vec<Message>,
    pub attachments: Vec<ContentPart>,
    pub scroll: u16,
    pub auto_scroll: bool,
    pub busy: bool,
    pub status: String,
    pub should_quit: bool,
    pub model: String,
    pub cwd: String,
    pub assistant_open: bool,
    pub pending_approval: Option<ApprovalRequest>,
    pub mode: Mode,
    pub reasoning: Reasoning,
    pub steering: Steering,
    pub lines: Vec<Line<'static>>,
    pub line_offsets: Vec<usize>,
    pub signatures: Vec<u64>,
    pub render_width: usize,
}

impl App {
    pub fn new(model: String, cwd: String, mode: Mode, reasoning: Reasoning) -> Self {
        Self {
            input: String::new(),
            items: Vec::new(),
            history: Vec::new(),
            attachments: Vec::new(),
            scroll: 0,
            auto_scroll: true,
            busy: false,
            status: "ready".to_string(),
            should_quit: false,
            model,
            cwd,
            assistant_open: false,
            pending_approval: None,
            mode,
            reasoning,
            steering: Steering::new(),
            lines: Vec::new(),
            line_offsets: Vec::new(),
            signatures: Vec::new(),
            render_width: 0,
        }
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
}
