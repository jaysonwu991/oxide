//! Interactive tool approval for every front-end.
//!
//! When a permission rule resolves to `ask`, the agent awaits this broker. The
//! broker emits an [`AgentEvent::ApprovalRequest`] carrying a request id, and
//! the front-end answers with [`ApprovalBroker::resolve`] (`deny`, `once`, or
//! `always`). `always` records a per-project rule in the [`ApprovalStore`] so
//! the prompt does not repeat for that tool. A request that never gets an answer
//! times out as a denial so a turn cannot hang forever.
//!
//! Emitting through the agent's own event stream (rather than a side channel)
//! keeps the request ordered after the `ToolCall` it belongs to, so a view can
//! pair the prompt with the tool card it already rendered.

use crate::agent::{AgentEvent, Approver, Steering};
use crate::approvals::ApprovalStore;
use crate::llm::Message;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

/// How long a request waits for an answer before it is denied.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

/// An answer to an approval request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Run the tool this once.
    Once,
    /// Run it and remember the tool for this project.
    Always,
    /// Refuse it. A message, when present, is handed to the agent as a user
    /// message so it can change course instead of retrying the same call.
    Deny { message: Option<String> },
}

impl Decision {
    /// Parses a front-end answer. `deny`/`once`/`always` are accepted; anything
    /// else is unknown, and an unknown answer is not applied.
    pub fn parse(decision: &str, message: Option<&str>) -> Option<Self> {
        match decision.trim().to_ascii_lowercase().as_str() {
            "deny" | "no" => Some(Decision::Deny {
                message: message
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string),
            }),
            "once" | "allow" | "yes" => Some(Decision::Once),
            "always" => Some(Decision::Always),
            _ => None,
        }
    }

    pub fn allows(&self) -> bool {
        match self {
            Decision::Once | Decision::Always => true,
            Decision::Deny { .. } => false,
        }
    }
}

struct Pending {
    sender: oneshot::Sender<Decision>,
}

pub struct ApprovalBroker {
    store: Mutex<ApprovalStore>,
    pending: Mutex<HashMap<u64, Pending>>,
    next: AtomicU64,
    timeout: Duration,
}

impl Default for ApprovalBroker {
    fn default() -> Self {
        Self::from_store(ApprovalStore::load(), APPROVAL_TIMEOUT)
    }
}

impl ApprovalBroker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn from_store(store: ApprovalStore, timeout: Duration) -> Self {
        Self {
            store: Mutex::new(store),
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            timeout,
        }
    }

    /// An `Approver` for the agent runtime that routes each request here. The
    /// event sender is the run's own stream, so the request arrives in order;
    /// `steering` receives a denial's message so the agent can read it.
    pub fn approver(
        self: &Arc<Self>,
        project: &Path,
        tx: UnboundedSender<AgentEvent>,
        steering: Steering,
    ) -> Approver {
        let broker = Arc::clone(self);
        let project = project.to_path_buf();
        Arc::new(move |tool, detail| {
            let broker = Arc::clone(&broker);
            let project = project.clone();
            let tx = tx.clone();
            let steering = steering.clone();
            Box::pin(async move { broker.request(project, tool, detail, tx, steering).await })
        })
    }

    async fn request(
        &self,
        project: PathBuf,
        tool: String,
        detail: String,
        tx: UnboundedSender<AgentEvent>,
        steering: Steering,
    ) -> bool {
        if self.allowed(&project, &tool) {
            return true;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending().insert(id, Pending { sender });
        if tx
            .send(AgentEvent::ApprovalRequest {
                id,
                tool: tool.clone(),
                detail,
            })
            .is_err()
        {
            self.pending().remove(&id);
            return false;
        }
        let decision = match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(decision)) => decision,
            _ => {
                self.pending().remove(&id);
                return false;
            }
        };
        if decision == Decision::Always {
            if let Err(err) = self.remember(&project, &tool) {
                eprintln!("could not save the approval rule for `{tool}`: {err:#}");
            }
        }
        if let Decision::Deny {
            message: Some(message),
        } = &decision
        {
            steering.push(Message::user(message.clone()));
        }
        decision.allows()
    }

    /// Resolves a pending request. `decision` is `deny`, `once`, or `always`;
    /// a `deny` may carry a message for the agent. Returns `false` when the id
    /// is unknown (a duplicate answer, or a request that already timed out).
    pub fn resolve(&self, id: u64, decision: &str, message: Option<&str>) -> bool {
        match Decision::parse(decision, message) {
            Some(decision) => self.answer(id, decision),
            None => false,
        }
    }

    /// Resolves a pending request from an in-process front-end, which has the
    /// decision already rather than a wire word. `false` means the id is
    /// unknown, so nothing was answered.
    pub fn answer(&self, id: u64, decision: Decision) -> bool {
        let Some(pending) = self.pending().remove(&id) else {
            return false;
        };
        pending.sender.send(decision).is_ok()
    }

    pub fn list(&self, project: &Path) -> Vec<String> {
        self.store().list(project)
    }

    pub fn clear(&self, project: &Path) -> anyhow::Result<()> {
        self.store().clear(project)
    }

    fn allowed(&self, project: &Path, tool: &str) -> bool {
        self.store().is_allowed(project, tool)
    }

    fn remember(&self, project: &Path, tool: &str) -> anyhow::Result<()> {
        self.store().allow(project, tool)
    }

    fn store(&self) -> std::sync::MutexGuard<'_, ApprovalStore> {
        self.store.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Pending>> {
        self.pending.lock().unwrap_or_else(|err| err.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn broker(name: &str, timeout: Duration) -> (Arc<ApprovalBroker>, PathBuf) {
        let dir = project(name);
        let store = ApprovalStore::load_from(dir.join("approvals.json"));
        (Arc::new(ApprovalBroker::from_store(store, timeout)), dir)
    }

    /// Drives one approval request: awaits the emitted event, answers with
    /// `decision`, and returns the approver's verdict.
    async fn ask(
        broker: &Arc<ApprovalBroker>,
        project: &Path,
        decision: &str,
        message: Option<&str>,
    ) -> (bool, u64, String) {
        let (tx, mut rx) = unbounded_channel();
        let steering = Steering::new();
        let approve = broker.approver(project, tx, steering);
        let requested = tokio::spawn(approve("bash".to_string(), "rm -rf /".to_string()));
        let event = rx.recv().await.expect("approval request");
        let (id, tool) = match event {
            AgentEvent::ApprovalRequest { id, tool, .. } => (id, tool),
            other => panic!("unexpected event: {other:?}"),
        };
        assert!(broker.resolve(id, decision, message));
        let allowed = requested.await.unwrap();
        (allowed, id, tool)
    }

    #[tokio::test]
    async fn answers_are_routed_to_the_request() {
        let (broker, dir) = broker("approval_once", APPROVAL_TIMEOUT);
        let (allowed, id, tool) = ask(&broker, &dir, "once", None).await;
        assert!(allowed);
        assert_eq!(id, 1);
        assert_eq!(tool, "bash");
        // "Once" is not remembered.
        assert!(broker.list(&dir).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn always_is_remembered_and_skips_the_next_prompt() {
        let (broker, dir) = broker("approval_always", APPROVAL_TIMEOUT);
        let (allowed, _, _) = ask(&broker, &dir, "always", None).await;
        assert!(allowed);
        assert_eq!(broker.list(&dir), vec!["bash".to_string()]);

        // The next request is answered from the saved rule, so no event is
        // emitted and no answer is needed.
        let (tx, mut rx) = unbounded_channel();
        let approve = broker.approver(&dir, tx, Steering::new());
        assert!(approve("bash".to_string(), "ls".to_string()).await);
        assert!(rx.try_recv().is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_denial_with_a_message_steers_the_agent() {
        let (broker, dir) = broker("approval_deny", APPROVAL_TIMEOUT);
        let (tx, mut rx) = unbounded_channel();
        let steering = Steering::new();
        let approve = broker.approver(&dir, tx, steering.clone());
        let requested = tokio::spawn(approve("bash".to_string(), "rm -rf /".to_string()));
        let id = match rx.recv().await.unwrap() {
            AgentEvent::ApprovalRequest { id, .. } => id,
            other => panic!("unexpected event: {other:?}"),
        };
        assert!(broker.resolve(id, "deny", Some("use a narrower command")));
        assert!(!requested.await.unwrap());

        let steered = steering.drain();
        assert_eq!(steered.len(), 1);
        assert_eq!(
            steered[0].display().as_deref(),
            Some("use a narrower command")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn an_unanswered_request_times_out_as_a_denial() {
        let (broker, dir) = broker("approval_timeout", Duration::from_millis(20));
        let (tx, mut rx) = unbounded_channel();
        let approve = broker.approver(&dir, tx, Steering::new());
        assert!(!approve("edit".to_string(), "a.rs".to_string()).await);
        // The stale request is dropped, so a late answer is ignored.
        let id = match rx.recv().await.unwrap() {
            AgentEvent::ApprovalRequest { id, .. } => id,
            other => panic!("unexpected event: {other:?}"),
        };
        assert!(!broker.resolve(id, "once", None));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_stream_that_is_gone_denies_instead_of_hanging() {
        let (broker, dir) = broker("approval_closed", APPROVAL_TIMEOUT);
        let (tx, rx) = unbounded_channel();
        drop(rx);
        let approve = broker.approver(&dir, tx, Steering::new());
        assert!(!approve("edit".to_string(), "a.rs".to_string()).await);
        assert!(broker.list(&dir).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn decisions_parse_from_a_front_end_answer() {
        assert_eq!(Decision::parse("once", None), Some(Decision::Once));
        assert_eq!(Decision::parse("Always", None), Some(Decision::Always));
        assert_eq!(
            Decision::parse("deny", Some(" no, use tabs ")),
            Some(Decision::Deny {
                message: Some("no, use tabs".to_string())
            })
        );
        // An empty message is the same as none.
        assert_eq!(
            Decision::parse("deny", Some("  ")),
            Some(Decision::Deny { message: None })
        );
        assert_eq!(Decision::parse("maybe", None), None);
    }
}
