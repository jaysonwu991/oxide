//! The tokio side of the window: one turn at a time, its events, and the two
//! questions a run can ask a person.
//!
//! The view never awaits anything; this module turns the UI's requests into
//! turns on the shared runtime and hands every event back over one channel, the
//! same shape the Electron harness uses between its two halves — except that the
//! window is here rather than at the other end of a pipe.

use oxide_core::agent::{AgentEvent, Approver, Cancel, Steering};
use oxide_core::approval::Decision;
use oxide_core::approvals::ApprovalStore;
use oxide_core::ask::{Answer, Asker, Question, Reply};
use oxide_core::changes::TurnChanges;
use oxide_core::llm::ContentPart;
use oxide_core::runner::TurnEnvironment;
use oxide_core::snapshots::Snapshots;
use oxide_core::updates::Check;
use oxide_desktop::turn::{notify_finished, open_session, start_turn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{oneshot, Mutex};

/// A request or an answer that has timed out waits this long, the same five
/// minutes every front-end gives it, so a run cannot hang on a dialog nobody
/// answers.
const ASK_TIMEOUT: Duration = Duration::from_secs(300);

/// Everything the runner tells the window.
#[derive(Debug)]
pub enum UiEvent {
    /// The run resolved its session and its title.
    Started {
        run: u64,
        session_id: Option<String>,
        title: String,
    },
    /// One event of the run's own stream.
    Agent { run: u64, event: AgentEvent },
    /// The run is over: what it changed, why it failed, or whether it was
    /// stopped.
    Ended {
        run: u64,
        session_id: Option<String>,
        error: Option<String>,
        stopped: bool,
        changes: Option<TurnChanges>,
    },
    /// A permission-gated tool is waiting on an answer.
    Approval {
        id: u64,
        tool: String,
        detail: String,
    },
    /// A skill's question is waiting on an answer.
    Question { id: u64, questions: Vec<Question> },
    /// Nobody answered in time, so the dialog stops offering an answer.
    QuestionClosed { id: u64 },
    /// The rules `/permissions` lists for a project.
    Permissions { rules: Vec<String> },
    /// The rules `/permissions clear` forgot, or why it could not.
    PermissionsCleared { error: Option<String> },
    /// What a check of this app's own release train found.
    Update { result: Result<Box<Check>, String> },
    /// Where an install put the bundle, or why it could not.
    UpdateInstalled { result: Result<PathBuf, String> },
}

/// The handles a run's mid-flight messages travel through, kept by the view so
/// Stop and the queue act on the run that is on screen.
#[derive(Clone)]
pub struct RunControls {
    pub cancel: Cancel,
    pub steering: Steering,
    pub follow_ups: Steering,
}

impl RunControls {
    pub fn new() -> Self {
        Self {
            cancel: Cancel::new(),
            steering: Steering::new(),
            follow_ups: Steering::new(),
        }
    }
}

impl Default for RunControls {
    fn default() -> Self {
        Self::new()
    }
}

/// A prompt about to become a turn.
#[derive(Debug, Clone)]
pub struct TurnRequest {
    pub run: u64,
    pub project: PathBuf,
    /// The thread to continue, or `None` for a new one.
    pub session: Option<String>,
    pub prompt: String,
    pub inline: Vec<ContentPart>,
}

#[derive(Clone)]
pub struct Runner {
    events: UnboundedSender<UiEvent>,
    runtime: tokio::runtime::Handle,
    approvals: Arc<Approvals>,
    questions: Arc<Questions>,
    /// The MCP servers, plugin host and language servers of a project, kept
    /// across its turns so an edit's language server is warm for the next one.
    environments: Arc<Mutex<HashMap<PathBuf, TurnEnvironment>>>,
}

impl Runner {
    pub fn new(runtime: tokio::runtime::Handle, events: UnboundedSender<UiEvent>) -> Self {
        Self {
            events,
            runtime,
            approvals: Approvals::new(),
            questions: Questions::new(),
            environments: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Starts a turn and hands back the handles it is steered with.
    pub fn start(&self, request: TurnRequest) -> RunControls {
        let controls = RunControls::new();
        let runner = self.clone();
        let handles = controls.clone();
        self.runtime.spawn(async move {
            runner.drive(request, handles).await;
        });
        controls
    }

    /// Answers a tool approval: once, always for this project, or not at all.
    pub fn answer_approval(&self, id: u64, decision: Decision) {
        let approvals = Arc::clone(&self.approvals);
        self.runtime
            .spawn(async move { approvals.answer(id, decision).await });
    }

    /// Answers a skill's question with the labels picked and the text typed, or
    /// nothing at all for a dialog that was dismissed.
    pub fn answer_question(&self, id: u64, answers: Vec<Answer>) {
        let questions = Arc::clone(&self.questions);
        self.runtime
            .spawn(async move { questions.answer(id, answers).await });
    }

    /// Lists the tools a project allows without asking, for `/permissions`.
    pub fn permissions(&self, project: PathBuf) {
        let approvals = Arc::clone(&self.approvals);
        let events = self.events.clone();
        let listed = project.clone();
        self.runtime.spawn(async move {
            let rules = approvals.list(&listed).await;
            let _ = project;
            let _ = events.send(UiEvent::Permissions { rules });
        });
    }

    /// Forgets every saved rule for a project, for `/permissions clear`.
    pub fn clear_permissions(&self, project: PathBuf) {
        let approvals = Arc::clone(&self.approvals);
        let events = self.events.clone();
        let listed = project.clone();
        self.runtime.spawn(async move {
            let cleared = approvals.clear(&listed).await;
            let _ = project;
            let _ = events.send(UiEvent::PermissionsCleared {
                error: cleared.err().map(|error| format!("{error:#}")),
            });
        });
    }

    /// Looks for the newest release of this app, on its own `gpui-v*` train.
    pub fn check_updates(&self) {
        let events = self.events.clone();
        self.runtime.spawn(async move {
            let result = crate::update::check()
                .await
                .map(Box::new)
                .map_err(|error| format!("{error:#}"));
            let _ = events.send(UiEvent::Update { result });
        });
    }

    /// Installs the release a check resolved, reporting where it landed.
    pub fn install_update(&self, check: Check) {
        let events = self.events.clone();
        self.runtime.spawn(async move {
            let result = crate::update::install(&check)
                .await
                .map_err(|error| format!("{error:#}"));
            let _ = events.send(UiEvent::UpdateInstalled { result });
        });
    }

    async fn drive(&self, request: TurnRequest, controls: RunControls) {
        let run = request.run;
        let cwd = request.project.clone();
        let events = self.events.clone();
        let log = match request.session.as_deref() {
            Some(reference) => open_session(&cwd, reference),
            None => open_session(&cwd, "new"),
        };
        let log = match log {
            Ok(log) => log,
            Err(error) => {
                let _ = events.send(UiEvent::Ended {
                    run,
                    session_id: None,
                    error: Some(format!("{error:#}")),
                    stopped: false,
                    changes: None,
                });
                return;
            }
        };
        // The state the run starts from, so the files it changes can be listed
        // once it ends. A folder the shadow snapshot refuses simply has no
        // listing rather than a failed turn.
        let baseline = tokio::task::spawn_blocking({
            let cwd = cwd.clone();
            move || Snapshots::baseline(&cwd)
        })
        .await
        .ok()
        .flatten();

        let approver = self.approvals.approver(cwd.clone(), self.events.clone());
        let asker = self.questions.asker(run, self.events.clone());
        let environment = self.environments.lock().await.get(&cwd).cloned();

        let started = start_turn(
            &cwd,
            &request.prompt,
            log,
            Some(approver),
            Some(asker),
            None,
            None,
            request.inline,
            environment,
            (
                controls.cancel.clone(),
                controls.steering.clone(),
                controls.follow_ups.clone(),
            ),
        )
        .await;
        let mut turn = match started {
            Ok(turn) => turn,
            Err(error) => {
                let _ = events.send(UiEvent::Ended {
                    run,
                    session_id: None,
                    error: Some(format!("{error:#}")),
                    stopped: false,
                    changes: None,
                });
                return;
            }
        };
        self.environments
            .lock()
            .await
            .insert(cwd.clone(), turn.environment.clone());
        let _ = events.send(UiEvent::Started {
            run,
            session_id: turn.session_id.clone(),
            title: turn.title.clone(),
        });

        let mut error = None;
        while let Some(event) = turn.events.recv().await {
            if let AgentEvent::Error(message) = &event {
                error = Some(message.clone());
            }
            let finished = matches!(event, AgentEvent::Finished(_));
            let _ = events.send(UiEvent::Agent { run, event });
            if finished {
                break;
            }
        }
        if let Err(failure) = turn.handle.await {
            if !controls.cancel.is_cancelled() {
                error.get_or_insert_with(|| format!("Agent task failed: {failure}"));
            }
        }

        let changes = match baseline {
            Some((snapshots, base)) => {
                tokio::task::spawn_blocking(move || snapshots.changes_since(&base).ok())
                    .await
                    .ok()
                    .flatten()
            }
            None => None,
        };
        self.questions.clear_run(run).await;
        let _ = events.send(UiEvent::Ended {
            run,
            session_id: turn.session_id.clone(),
            error,
            stopped: controls.cancel.is_cancelled(),
            changes,
        });
        notify_finished(
            &cwd,
            turn.session_id.as_deref(),
            controls.cancel.is_cancelled(),
        );
    }
}

/// Tool approvals: an `Approver` that announces the call and waits for the
/// window's answer. `always` records the tool in the shared `approvals.json`, so
/// the terminal, the Electron window and this one stop asking for it.
/// A tool approval waiting on the window: the answer's channel, and what
/// `always` would record.
struct ApprovalRequest {
    sender: oneshot::Sender<bool>,
    project: PathBuf,
    tool: String,
}

pub struct Approvals {
    pending: std::sync::Mutex<HashMap<u64, ApprovalRequest>>,
    next: AtomicU64,
    store: Mutex<ApprovalStore>,
}

impl Approvals {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: std::sync::Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            store: Mutex::new(ApprovalStore::load()),
        })
    }

    pub fn approver(
        self: &Arc<Self>,
        project: PathBuf,
        events: UnboundedSender<UiEvent>,
    ) -> Approver {
        let broker = Arc::clone(self);
        Arc::new(move |tool, detail| {
            let broker = Arc::clone(&broker);
            let project = project.clone();
            let events = events.clone();
            Box::pin(async move { broker.request(project, tool, detail, events).await })
        })
    }

    async fn request(
        &self,
        project: PathBuf,
        tool: String,
        detail: String,
        events: UnboundedSender<UiEvent>,
    ) -> bool {
        if self.store.lock().await.is_allowed(&project, &tool) {
            return true;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending().insert(
            id,
            ApprovalRequest {
                sender,
                project: project.clone(),
                tool: tool.clone(),
            },
        );
        if events.send(UiEvent::Approval { id, tool, detail }).is_err() {
            self.pending().remove(&id);
            return false;
        }
        match tokio::time::timeout(ASK_TIMEOUT, receiver).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending().remove(&id);
                false
            }
        }
    }

    pub async fn answer(&self, id: u64, decision: Decision) {
        let Some(pending) = self.pending().remove(&id) else {
            return;
        };
        if decision == Decision::Always {
            let _ = self
                .store
                .lock()
                .await
                .allow(&pending.project, &pending.tool);
        }
        let _ = pending.sender.send(decision.allows());
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<u64, ApprovalRequest>> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub async fn list(&self, project: &std::path::Path) -> Vec<String> {
        self.store.lock().await.list(project)
    }

    pub async fn clear(&self, project: &std::path::Path) -> anyhow::Result<()> {
        self.store.lock().await.clear(project)
    }
}

struct PendingQuestion {
    run: u64,
    sender: oneshot::Sender<Vec<Answer>>,
}

/// A skill's questions: an `Asker` that announces the request and waits for the
/// window's answer, giving up after the shared timeout and saying so.
pub struct Questions {
    pending: Mutex<HashMap<u64, PendingQuestion>>,
    next: AtomicU64,
}

impl Questions {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        })
    }

    pub fn asker(self: &Arc<Self>, run: u64, events: UnboundedSender<UiEvent>) -> Asker {
        let broker = Arc::clone(self);
        Arc::new(move |questions: Vec<Question>| {
            let broker = Arc::clone(&broker);
            let events = events.clone();
            Box::pin(async move { broker.request(run, questions, events).await })
        })
    }

    async fn request(
        &self,
        run: u64,
        questions: Vec<Question>,
        events: UnboundedSender<UiEvent>,
    ) -> Option<Reply> {
        if questions.is_empty() {
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .await
            .insert(id, PendingQuestion { run, sender });
        if events.send(UiEvent::Question { id, questions }).is_err() {
            self.pending.lock().await.remove(&id);
            return None;
        }
        match tokio::time::timeout(ASK_TIMEOUT, receiver).await {
            Ok(Ok(answers)) if answers.is_empty() => Some(Reply::Dismissed),
            Ok(Ok(answers)) => Some(Reply::Answers(answers)),
            _ => {
                self.pending.lock().await.remove(&id);
                let _ = events.send(UiEvent::QuestionClosed { id });
                None
            }
        }
    }

    pub async fn answer(&self, id: u64, answers: Vec<Answer>) {
        if let Some(pending) = self.pending.lock().await.remove(&id) {
            let _ = pending.sender.send(answers);
        }
    }

    /// A run that ended takes its own unanswered questions with it, so they do
    /// not sit against the next run's id space until each times out.
    pub async fn clear_run(&self, run: u64) {
        self.pending
            .lock()
            .await
            .retain(|_, pending| pending.run != run);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_denial_is_not_an_allowance() {
        assert!(!Decision::Deny { message: None }.allows());
        assert!(Decision::Once.allows());
        assert!(Decision::Always.allows());
    }

    #[test]
    fn a_question_the_window_dismissed_is_an_empty_answer_list() {
        // The view sends an empty list for Dismiss, which is what the broker
        // reads as `Reply::Dismissed` rather than as a set of blank answers.
        let answers: Vec<Answer> = Vec::new();
        assert!(answers.is_empty());
    }
}
