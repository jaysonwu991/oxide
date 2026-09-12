use crate::agent::ApprovalRequest;
use crate::llm::{ContentPart, Message};

#[derive(Debug, Clone)]
pub enum ChatItem {
    User(String),
    Assistant(String),
    Tool { name: String, args: String },
    ToolResult { name: String, output: String },
    Error(String),
    Info(String),
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
}

impl App {
    pub fn new(model: String, cwd: String) -> Self {
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
