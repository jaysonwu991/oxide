//! One thread's state and the window's lists, with no GPUI types in them.
//!
//! The view paints what this module holds; `runner` feeds it the agent's events
//! and the window's dialogs are opened from them. Keeping the state machine free
//! of the framework is what lets a turn's transcript be exercised without a
//! window, the way the Electron harness's own modules are.

use crate::transcript::{self, Item, ToolItem};
use oxide_core::agent::AgentEvent;
use oxide_core::ask::Answer;
use oxide_core::ask::Question;
use oxide_core::at::{self, AtSuggestion};
use oxide_core::changes::{ChangeStatus, TurnChanges};
use oxide_core::commands::CommandEntry;
use oxide_core::llm::{Message, MessageContent};
use oxide_desktop::manager::{ProjectView, SessionView};
use std::path::PathBuf;

/// The chat's running totals, added up the way the terminal's footer adds them:
/// every `usage` event is one step of the conversation, so the tokens are the
/// chat's rather than the newest request's, while the context is the prompt the
/// newest request was sent with.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    /// The newest request's cache reads over its own prompt, in percent, with a
    /// step that read no cache keeping the warm one's rate.
    pub cache_hit: Option<f64>,
    /// The prompt tokens of the newest request: the context the thread was last
    /// run with.
    pub context: Option<u64>,
}

impl Usage {
    pub fn record(&mut self, event: &AgentEvent) {
        let AgentEvent::Usage {
            input,
            output,
            cache_read,
            cache_write,
            cost,
            context_tokens,
        } = event
        else {
            return;
        };
        self.input += input;
        self.output += output;
        self.cache_read += cache_read;
        self.cache_write += cache_write;
        self.cost += cost;
        if let Some(tokens) = context_tokens {
            self.context = Some(*tokens);
        }
        let prompt = input + cache_read + cache_write;
        if prompt > 0 && cache_read + cache_write > 0 {
            self.cache_hit = Some(*cache_read as f64 / prompt as f64 * 100.0);
        }
    }

    /// How much of the model's window the newest request filled, or `None`
    /// before the thread has been run.
    pub fn context_percent(&self, window: u64) -> Option<f64> {
        let tokens = self.context?;
        (window > 0).then(|| tokens as f64 / window as f64 * 100.0)
    }
}

/// What the composer's corner is doing, in the words the status line carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Starting,
    Thinking,
    Writing,
    Tool,
    Stopped,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Idle => "Ready",
            Phase::Starting => "Starting",
            Phase::Thinking => "Thinking",
            Phase::Writing => "Responding",
            Phase::Tool => "Running a tool",
            Phase::Stopped => "Stopped",
        }
    }
}

/// One conversation on screen: its thread, its transcript and its totals.
#[derive(Debug, Default)]
pub struct Thread {
    pub project: PathBuf,
    pub session_id: Option<String>,
    pub title: String,
    pub items: Vec<Item>,
    pub usage: Usage,
    pub running: bool,
    pub phase: Phase,
    pub error: Option<String>,
    pub changes: Option<TurnChanges>,
    /// Messages typed while the turn ran, in the order they will be delivered.
    pub queued: Vec<String>,
}

impl Thread {
    pub fn new(project: PathBuf) -> Self {
        Self {
            project,
            title: "New chat".to_string(),
            ..Default::default()
        }
    }

    /// The label the header and the sidebar show, which is the thread's own
    /// summarized title once it has one.
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() {
            "New chat"
        } else {
            &self.title
        }
    }

    pub fn push_user(&mut self, text: impl Into<String>) {
        self.items.push(Item::User(text.into()));
    }

    /// The turn is over: nothing is still being written, and the phase goes back
    /// to idle unless the run was stopped.
    pub fn finish(&mut self) {
        self.running = false;
        for item in &mut self.items {
            if let Item::Assistant { streaming, .. } = item {
                *streaming = false;
            }
            if let Item::Tool(tool) = item {
                tool.running = false;
            }
        }
        if self.phase != Phase::Stopped {
            self.phase = Phase::Idle;
        }
    }

    pub fn status(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        self.phase.label().to_string()
    }

    /// Applies one event of the run into the transcript, the totals and the
    /// phase. An `ApprovalRequest` or `QuestionRequest` is the window's own to
    /// paint, so it is not part of the transcript.
    pub fn apply(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Text(text) => {
                match self.items.last_mut() {
                    Some(Item::Assistant {
                        text: out,
                        streaming,
                    }) => {
                        out.push_str(text);
                        *streaming = true;
                    }
                    _ => self.items.push(Item::Assistant {
                        text: text.clone(),
                        streaming: true,
                    }),
                }
                self.phase = Phase::Writing;
            }
            AgentEvent::ThinkingDelta(text) => {
                match self.items.last_mut() {
                    Some(Item::Thinking {
                        text: out,
                        millis: None,
                    }) => out.push_str(text),
                    _ => self.items.push(Item::Thinking {
                        text: text.clone(),
                        millis: None,
                    }),
                }
                self.phase = Phase::Thinking;
            }
            AgentEvent::Thought { millis } | AgentEvent::ThoughtDone { millis } => {
                if let Some(Item::Thinking {
                    millis: elapsed, ..
                }) = self.items.last_mut()
                {
                    *elapsed = Some(*millis);
                }
            }
            AgentEvent::Retrying {
                attempt,
                max,
                delay_ms,
            } => {
                // The failed attempt's output is abandoned, so the item it was
                // writing is dropped and the retry starts fresh.
                while self
                    .items
                    .last()
                    .is_some_and(|item| item.streaming() && !matches!(item, Item::Tool(_)))
                {
                    self.items.pop();
                }
                self.items.push(Item::Info(format!(
                    "Retrying ({attempt}/{max}) in {:.1}s…",
                    *delay_ms as f64 / 1000.0
                )));
            }
            AgentEvent::ToolCall { name, args } => {
                self.items
                    .push(Item::Tool(ToolItem::new(name.clone(), args.clone())));
                self.phase = Phase::Tool;
            }
            AgentEvent::ToolProgress { chunk, .. } => {
                if let Some(Item::Tool(tool)) = self.items.last_mut() {
                    tool.output.push_str(chunk);
                }
            }
            AgentEvent::ToolResult {
                name,
                args,
                output,
                is_error,
                millis,
                ..
            } => {
                let settled = match self.items.last_mut() {
                    Some(Item::Tool(tool)) if tool.running && tool.name == *name => Some(tool),
                    _ => None,
                };
                match settled {
                    Some(tool) => {
                        tool.output = output.clone();
                        tool.is_error = *is_error;
                        tool.millis = *millis;
                        tool.running = false;
                    }
                    None => {
                        let mut tool = ToolItem::new(name.clone(), args.clone());
                        tool.output = output.clone();
                        tool.is_error = *is_error;
                        tool.millis = *millis;
                        tool.running = false;
                        self.items.push(Item::Tool(tool));
                    }
                }
                self.phase = Phase::Writing;
            }
            AgentEvent::SubagentActivity { agent, tool, args } => {
                self.items.push(Item::Info(format!(
                    "↳ {agent} · {} {}",
                    transcript::tool_label(tool),
                    transcript::tool_summary(tool, args)
                )));
            }
            AgentEvent::Steered { label, .. } => {
                self.push_user(label.clone());
                self.queued.clear();
            }
            AgentEvent::SessionTitle { title } => self.title = title.clone(),
            AgentEvent::Usage { .. } => self.usage.record(event),
            AgentEvent::ContextWindow {
                tokens,
                provider,
                model,
            } => self.items.push(Item::Info(format!(
                "Context window is {tokens} tokens for {provider}/{model}"
            ))),
            AgentEvent::Compaction {
                summarized,
                summary,
                ..
            } => self.items.push(Item::Info(format!(
                "[compaction] {summarized} messages summarized\n{summary}"
            ))),
            AgentEvent::Branch { message, .. } => self.items.push(Item::Info(message.clone())),
            AgentEvent::Error(error) => {
                self.error = Some(error.clone());
                self.items.push(Item::Error(error.clone()));
            }
            AgentEvent::Finished(_) => self.finish(),
            AgentEvent::ApprovalRequest { .. } | AgentEvent::QuestionRequest { .. } => {}
            AgentEvent::QuestionClosed { .. } => {}
        }
    }
}

/// What a thread that was reopened already said. The store records what a call
/// found only through its result, so a replayed card is neutral rather than
/// claiming a state it cannot know.
pub fn replay(messages: &[Message]) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    // The call each stored result belongs to, so a card settles into the call
    // that made it rather than into whichever card happens to be last.
    let mut calls: Vec<(String, usize)> = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "user" => {
                if let Some(text) = message.content.as_ref().map(MessageContent::display) {
                    items.push(Item::User(text));
                }
            }
            "assistant" => {
                if let Some(text) = message
                    .content
                    .as_ref()
                    .map(MessageContent::display)
                    .filter(|text| !text.trim().is_empty())
                {
                    items.push(Item::Assistant {
                        text,
                        streaming: false,
                    });
                }
                for call in message.tool_calls.iter().flatten() {
                    let index = items.len();
                    calls.push((call.id.clone(), index));
                    items.push(Item::Tool(ToolItem::new(
                        call.function.name.clone(),
                        call.function.arguments.clone(),
                    )));
                }
            }
            "tool" => {
                let output = message
                    .content
                    .as_ref()
                    .map(MessageContent::display)
                    .unwrap_or_default();
                let index = message
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| calls.iter().rev().find(|(call, _)| call == id))
                    .map(|(_, index)| *index)
                    .or_else(|| calls.last().map(|(_, index)| *index));
                if let Some(Item::Tool(tool)) = index.and_then(|index| items.get_mut(index)) {
                    tool.output = output;
                    tool.running = false;
                }
            }
            _ => {}
        }
    }
    // A call the store has no result for — a run that was killed mid-tool — is
    // painted as a card that finished, since there is nothing left to wait on.
    for item in &mut items {
        if let Item::Tool(tool) = item {
            tool.running = false;
        }
    }
    items
}

/// A project's row in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    pub sessions: usize,
}

/// A thread's row under its project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRow {
    pub id: String,
    pub title: String,
    pub modified_at: u64,
    pub pinned: bool,
}

pub fn project_rows(projects: &[ProjectView]) -> Vec<ProjectRow> {
    projects
        .iter()
        .map(|project| ProjectRow {
            id: project.id.clone(),
            path: PathBuf::from(&project.path),
            name: project.name.clone(),
            sessions: project.session_count,
        })
        .collect()
}

pub fn thread_rows(sessions: &[SessionView]) -> Vec<ThreadRow> {
    sessions
        .iter()
        .map(|session| ThreadRow {
            id: session.summary.id.clone(),
            title: session
                .summary
                .name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| session.summary.preview.clone()),
            modified_at: session.summary.modified_at,
            pinned: session.pinned,
        })
        .collect()
}

/// The `/` menu's rows for what has been typed: a row whose name starts with the
/// query, then one whose description mentions it, so `/co` offers the commands
/// before the descriptions that happen to hold the word.
pub fn filter_commands<'a>(query: &str, entries: &'a [CommandEntry]) -> Vec<&'a CommandEntry> {
    let query = query.trim().trim_start_matches('/').to_lowercase();
    if query.is_empty() {
        return entries.iter().collect();
    }
    let mut ranked: Vec<(u8, &CommandEntry)> = entries
        .iter()
        .filter_map(|entry| {
            let name = entry.name.to_lowercase();
            if name.starts_with(&query) {
                Some((0, entry))
            } else if name.contains(&query) {
                Some((1, entry))
            } else if entry.description.to_lowercase().contains(&query) {
                Some((2, entry))
            } else {
                None
            }
        })
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    ranked.into_iter().map(|(_, entry)| entry).collect()
}

/// The status mark a changed file wears, the one git prints.
pub fn change_letter(status: ChangeStatus) -> char {
    status.letter()
}

/// A skill's question as the window's dialog holds it: which step is on screen,
/// what has been picked and what has been typed. Kept out of the view so the
/// answers a form sends back can be checked without a window.
#[derive(Debug, Clone)]
pub struct QuestionForm {
    pub questions: Vec<Question>,
    pub step: usize,
    pub picked: Vec<Vec<usize>>,
    pub typed: Vec<String>,
}

impl QuestionForm {
    pub fn new(questions: Vec<Question>) -> Self {
        let count = questions.len();
        Self {
            picked: vec![Vec::new(); count],
            typed: vec![String::new(); count],
            questions,
            step: 0,
        }
    }

    pub fn last_step(&self) -> bool {
        self.step + 1 >= self.questions.len()
    }

    /// Picks one label, or toggles it when the question takes more than one.
    pub fn choose(&mut self, index: usize) {
        let Some(question) = self.questions.get(self.step) else {
            return;
        };
        let multi = question.multi_select;
        let picked = &mut self.picked[self.step];
        if multi {
            match picked.iter().position(|value| *value == index) {
                Some(position) => {
                    picked.remove(position);
                }
                None => picked.push(index),
            }
        } else {
            picked.clear();
            picked.push(index);
        }
    }

    pub fn is_picked(&self, index: usize) -> bool {
        self.picked
            .get(self.step)
            .is_some_and(|picked| picked.contains(&index))
    }

    /// What the model reads back: the labels picked and the words typed, in the
    /// order the questions were asked, and nothing at all for a form nobody
    /// filled in — which the broker reads as a dismissal.
    pub fn answers(&self) -> Vec<Answer> {
        let mut answers = Vec::new();
        for (index, question) in self.questions.iter().enumerate() {
            let mut values: Vec<String> = self.picked[index]
                .iter()
                .filter_map(|picked| question.options.get(*picked))
                .map(|choice| choice.label.clone())
                .collect();
            let typed = self.typed[index].trim();
            if !typed.is_empty() {
                values.push(typed.to_string());
            }
            if !values.is_empty() {
                answers.push(Answer {
                    question: question.question.clone(),
                    values,
                });
            }
        }
        answers
    }
}

/// The list above the composer: the paths for an `@` reference under the caret,
/// or the commands for a `/` the message starts with.
#[derive(Debug, Clone, Default)]
pub enum Completion {
    #[default]
    None,
    At {
        start: usize,
        end: usize,
        rows: Vec<AtSuggestion>,
        selected: usize,
    },
    Slash {
        start: usize,
        end: usize,
        rows: Vec<String>,
        selected: usize,
    },
}

impl Completion {
    pub fn is_open(&self) -> bool {
        !matches!(self, Completion::None)
    }

    pub fn len(&self) -> usize {
        match self {
            Completion::None => 0,
            Completion::At { rows, .. } => rows.len(),
            Completion::Slash { rows, .. } => rows.len(),
        }
    }

    /// Walks the highlight, wrapping at either end.
    pub fn move_by(&mut self, delta: i32) {
        let len = self.len();
        if len == 0 {
            return;
        }
        let selected = match self {
            Completion::None => return,
            Completion::At { selected, .. } | Completion::Slash { selected, .. } => selected,
        };
        *selected = (*selected as i32 + delta).rem_euclid(len as i32) as usize;
    }

    /// The text a taken row inserts, and the range it replaces.
    pub fn take(&self) -> Option<(std::ops::Range<usize>, String)> {
        match self {
            Completion::None => None,
            Completion::At {
                start,
                end,
                rows,
                selected,
            } => {
                let row = rows.get(*selected)?;
                Some((*start..*end, row.insert.clone()))
            }
            Completion::Slash {
                start,
                end,
                rows,
                selected,
            } => {
                let name = rows.get(*selected)?;
                Some((*start..*end, format!("/{name} ")))
            }
        }
    }

    pub fn range(&self) -> Option<std::ops::Range<usize>> {
        match self {
            Completion::None => None,
            Completion::At { start, end, .. } | Completion::Slash { start, end, .. } => {
                Some(*start..*end)
            }
        }
    }
}

/// The rows `/` offers for what has been typed so far, cut to the list's own
/// size, or nothing when the caret is past the command word.
pub fn slash_completion(text: &str, entries: &[CommandEntry], limit: usize) -> Completion {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return Completion::None;
    };
    if rest.contains(char::is_whitespace) {
        return Completion::None;
    }
    let rows: Vec<String> = filter_commands(rest, entries)
        .into_iter()
        .take(limit)
        .map(|entry| entry.name.clone())
        .collect();
    if rows.is_empty() {
        return Completion::None;
    }
    Completion::Slash {
        start: text.len() - trimmed.len(),
        end: text.len(),
        rows,
        selected: 0,
    }
}

/// The rows `@` offers for the reference the caret sits in.
pub fn at_completion(paths: &[String], text: &str, cursor: usize, limit: usize) -> Completion {
    let Some(token) = at::token(text, cursor) else {
        return Completion::None;
    };
    let rows = at::suggestions(paths, Some(&token), limit);
    if rows.is_empty() {
        return Completion::None;
    }
    Completion::At {
        start: token.start,
        end: token.end,
        rows,
        selected: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::ask::Question;
    use oxide_core::changes::FileChange;
    use oxide_core::commands::builtin_entries_for;
    use oxide_core::commands::FrontEnd;
    use oxide_core::llm::{FunctionCall, ToolCall};

    fn usage_event(input: u64, output: u64, cache_read: u64, context: Option<u64>) -> AgentEvent {
        AgentEvent::Usage {
            input,
            output,
            cache_read,
            cache_write: 0,
            cost: 0.25,
            context_tokens: context,
        }
    }

    #[test]
    fn a_streamed_answer_is_one_item() {
        let mut thread = Thread::default();
        thread.apply(&AgentEvent::Text("Hel".into()));
        thread.apply(&AgentEvent::Text("lo".into()));
        assert_eq!(
            thread.items,
            vec![Item::Assistant {
                text: "Hello".into(),
                streaming: true
            }]
        );
        thread.apply(&AgentEvent::Finished(vec![]));
        assert_eq!(thread.phase, Phase::Idle);
        assert!(matches!(
            thread.items[0],
            Item::Assistant {
                streaming: false,
                ..
            }
        ));
    }

    #[test]
    fn a_reasoning_stream_is_its_own_block_and_records_its_duration() {
        let mut thread = Thread::default();
        thread.apply(&AgentEvent::ThinkingDelta("weigh".into()));
        thread.apply(&AgentEvent::ThinkingDelta("ing".into()));
        thread.apply(&AgentEvent::Thought { millis: 1200 });
        assert_eq!(
            thread.items,
            vec![Item::Thinking {
                text: "weighing".into(),
                millis: Some(1200)
            }]
        );
    }

    #[test]
    fn a_retry_drops_the_abandoned_answer_rather_than_extending_it() {
        let mut thread = Thread::default();
        thread.apply(&AgentEvent::ThinkingDelta("hmm".into()));
        thread.apply(&AgentEvent::Text("partial".into()));
        thread.apply(&AgentEvent::Retrying {
            attempt: 1,
            max: 3,
            delay_ms: 2000,
        });
        assert_eq!(thread.items.len(), 1);
        assert!(matches!(thread.items[0], Item::Info(_)));
        thread.apply(&AgentEvent::Text("fresh".into()));
        assert!(matches!(
            thread.items[1],
            Item::Assistant { ref text, .. } if text == "fresh"
        ));
    }

    #[test]
    fn a_tool_call_settles_into_the_card_it_opened() {
        let mut thread = Thread::default();
        thread.apply(&AgentEvent::ToolCall {
            name: "bash".into(),
            args: r#"{"command":"ls"}"#.into(),
        });
        thread.apply(&AgentEvent::ToolProgress {
            name: "bash".into(),
            chunk: "a\n".into(),
        });
        thread.apply(&AgentEvent::ToolResult {
            name: "bash".into(),
            args: r#"{"command":"ls"}"#.into(),
            output: "a\nb\n".into(),
            is_error: false,
            diff: None,
            millis: 42,
        });
        assert_eq!(thread.items.len(), 1);
        let Item::Tool(tool) = &thread.items[0] else {
            panic!("a tool card is what a call and its result make");
        };
        assert_eq!(tool.output, "a\nb\n");
        assert!(!tool.running);
        assert_eq!(tool.millis, 42);
    }

    #[test]
    fn a_result_without_its_call_still_reads() {
        let mut thread = Thread::default();
        thread.apply(&AgentEvent::ToolResult {
            name: "read".into(),
            args: r#"{"path":"a.rs"}"#.into(),
            output: "text".into(),
            is_error: true,
            diff: None,
            millis: 3,
        });
        assert!(matches!(&thread.items[0], Item::Tool(tool) if tool.is_error && !tool.running));
    }

    #[test]
    fn totals_add_up_across_steps_and_the_context_is_the_newest_prompt() {
        let mut usage = Usage::default();
        usage.record(&usage_event(1000, 200, 0, Some(1000)));
        usage.record(&usage_event(2000, 300, 500, Some(2500)));
        assert_eq!(usage.input, 3000);
        assert_eq!(usage.output, 500);
        assert_eq!(usage.context, Some(2500));
        assert!(usage.cost > 0.49);
        // The second step read 500 of a 2500-token prompt.
        assert!((usage.cache_hit.unwrap() - 20.0).abs() < 1e-9);
        assert!((usage.context_percent(10_000).unwrap() - 25.0).abs() < 1e-9);
    }

    #[test]
    fn a_step_that_reads_no_cache_keeps_the_warm_ones_rate() {
        let mut usage = Usage::default();
        usage.record(&usage_event(1000, 10, 1000, None));
        let warm = usage.cache_hit.unwrap();
        usage.record(&usage_event(1000, 10, 0, None));
        assert_eq!(usage.cache_hit, Some(warm));
    }

    #[test]
    fn a_thread_with_no_run_has_no_context_reading() {
        let usage = Usage::default();
        assert_eq!(usage.context_percent(128_000), None);
        assert_eq!(usage.context_percent(0), None);
    }

    #[test]
    fn a_finished_turn_says_what_it_was_and_what_went_wrong() {
        let mut thread = Thread::default();
        thread.apply(&AgentEvent::Error("no API key".into()));
        assert_eq!(thread.status(), "no API key");
        thread.error = None;
        thread.phase = Phase::Writing;
        assert_eq!(thread.status(), "Responding");
        thread.finish();
        assert_eq!(thread.status(), "Ready");
    }

    #[test]
    fn a_stopped_turn_keeps_its_phase() {
        let mut thread = Thread {
            running: true,
            phase: Phase::Stopped,
            ..Default::default()
        };
        thread.finish();
        assert_eq!(thread.status(), "Stopped");
        assert!(!thread.running);
    }

    #[test]
    fn a_reopened_thread_replays_what_it_said_and_called() {
        let messages = vec![
            Message {
                role: "user".into(),
                content: Some(MessageContent::Text("fix it".into())),
                tool_calls: None,
                tool_call_id: None,
                thinking: None,
            },
            Message {
                role: "assistant".into(),
                content: Some(MessageContent::Text("looking".into())),
                tool_calls: Some(vec![ToolCall {
                    id: "call-1".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "read".into(),
                        arguments: r#"{"path":"a.rs"}"#.into(),
                    },
                    signature: None,
                }]),
                tool_call_id: None,
                thinking: None,
            },
            Message {
                role: "tool".into(),
                content: Some(MessageContent::Text("fn main() {}".into())),
                tool_calls: None,
                tool_call_id: Some("call-1".into()),
                thinking: None,
            },
        ];
        let items = replay(&messages);
        assert_eq!(items[0], Item::User("fix it".into()));
        assert_eq!(
            items[1],
            Item::Assistant {
                text: "looking".into(),
                streaming: false
            }
        );
        let Item::Tool(tool) = &items[2] else {
            panic!("the call is its own card");
        };
        assert_eq!(tool.name, "read");
        assert_eq!(tool.output, "fn main() {}");
        assert!(!tool.running);
        assert!(!tool.is_error);
    }

    #[test]
    fn an_empty_assistant_turn_is_not_a_bubble() {
        let messages = vec![Message {
            role: "assistant".into(),
            content: Some(MessageContent::Text("  ".into())),
            tool_calls: None,
            tool_call_id: None,
            thinking: None,
        }];
        assert!(replay(&messages).is_empty());
    }

    #[test]
    fn the_slash_menu_ranks_names_before_descriptions() {
        let entries = builtin_entries_for(FrontEnd::Desktop);
        let rows = filter_commands("/se", &entries);
        assert_eq!(rows.first().map(|row| row.name.as_str()), Some("session"));
        let all = filter_commands("", &entries);
        assert_eq!(all.len(), entries.len());
        // A word the catalog does not declare matches nothing here.
        assert!(filter_commands("/mcps", &entries).is_empty());
    }

    #[test]
    fn a_changed_file_wears_the_letter_git_gives_it() {
        assert_eq!(change_letter(ChangeStatus::Added), 'A');
        assert_eq!(change_letter(ChangeStatus::Modified), 'M');
        assert_eq!(change_letter(ChangeStatus::Deleted), 'D');
    }

    #[test]
    fn a_project_and_its_threads_become_rows() {
        let projects = vec![ProjectView {
            id: "/tmp/a".into(),
            path: "/tmp/a".into(),
            display_path: "~/a".into(),
            name: "a".into(),
            registered: true,
            exists: true,
            session_count: 2,
            last_session_at: 0,
            last_opened_at: None,
            pinned: false,
        }];
        let rows = project_rows(&projects);
        assert_eq!(rows[0].name, "a");
        assert_eq!(rows[0].path, PathBuf::from("/tmp/a"));
        assert_eq!(rows[0].sessions, 2);
    }

    #[test]
    fn a_question_is_not_part_of_the_transcript() {
        let mut thread = Thread::default();
        thread.apply(&AgentEvent::QuestionRequest {
            id: 1,
            questions: vec![Question {
                question: "Which?".into(),
                header: None,
                options: vec![],
                multi_select: false,
            }],
        });
        assert!(thread.items.is_empty());
    }

    #[test]
    fn a_turn_that_changed_files_lists_them() {
        let changes = TurnChanges {
            files: vec![FileChange {
                path: "src/main.rs".into(),
                status: ChangeStatus::Modified,
                added: 3,
                removed: 1,
                binary: false,
                diff: String::new(),
            }],
            added: 3,
            removed: 1,
        };
        assert!(!changes.is_empty());
        assert_eq!(changes.files[0].path, "src/main.rs");
    }
}
