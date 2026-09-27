//! Interactive questions for the desktop.
//!
//! When the model follows a skill that asks something only the user can answer,
//! it calls the `ask` tool and the agent awaits this broker. The broker emits a
//! `question-request` event carrying a request id and the questions, and the UI
//! answers with the `resolve_question` command: the values the user picked or
//! typed, or nothing at all for a dismissed dialog. A request that never gets an
//! answer times out as a dismissal so a turn cannot hang forever, and the UI is
//! told with a `question-closed` event so a dialog stops offering an answer
//! nothing is waiting for.
//!
//! The broker outlives a turn — the window keeps one — so every request records
//! the turn that asked: a turn that is stopped or crashes takes its own pending
//! requests with it when [`AskBroker::clear_run`] runs, rather than leaving them
//! to age out against the cap.

use oxide_core::ask::{Answer, Asker, Question, Reply};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::{oneshot, Mutex};

const ASK_TIMEOUT: Duration = Duration::from_secs(300);

/// How many questions wait at once: more than one `ask` call can ask, with the
/// headroom a second request needs if the model asks again before the first is
/// answered. Entries are removed when they are answered, when they time out, and
/// when the turn that asked ends.
const MAX_PENDING: usize = 32;

struct Pending {
    /// The turn that asked, so it can take its own requests with it.
    run: u64,
    sender: oneshot::Sender<Vec<Answer>>,
}

pub struct AskBroker {
    app: AppHandle,
    pending: Mutex<HashMap<u64, Pending>>,
    next: AtomicU64,
}

impl AskBroker {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }
    }

    /// An `Asker` for the agent runtime of the turn numbered `run`, so a
    /// request belongs to the turn that asked it.
    pub fn asker_for(self: &Arc<Self>, run: u64) -> Asker {
        let broker = Arc::clone(self);
        Arc::new(move |questions| {
            let broker = Arc::clone(&broker);
            Box::pin(async move { broker.request(run, questions).await })
        })
    }

    async fn request(&self, run: u64, questions: Vec<Question>) -> Option<Reply> {
        if questions.is_empty() {
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.len() >= MAX_PENDING {
                return None;
            }
            pending.insert(id, Pending { run, sender: tx });
        }
        if self
            .app
            .emit(
                "question-request",
                json!({ "id": id, "questions": questions }),
            )
            .is_err()
        {
            self.pending.lock().await.remove(&id);
            return None;
        }
        match tokio::time::timeout(ASK_TIMEOUT, rx).await {
            // An answer with nothing in it is a dialog the user dismissed.
            Ok(Ok(answers)) if answers.is_empty() => Some(Reply::Dismissed),
            Ok(Ok(answers)) => Some(Reply::Answers(answers)),
            // Nobody answered in time: the dialog is told to close, and the
            // request is gone either way.
            Err(_) => {
                self.pending.lock().await.remove(&id);
                let _ = self.app.emit("question-closed", json!({ "id": id }));
                None
            }
            // The sender was dropped by a turn that ended, which already told
            // the UI to close its dialog.
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                None
            }
        }
    }

    /// Forgets every request the turn numbered `run` is waiting on, which is
    /// what its end has to do: the run is gone (answered elsewhere, stopped, or
    /// aborted), so its requests can never be answered and would otherwise sit
    /// against [`MAX_PENDING`] until each one timed out.
    pub async fn clear_run(&self, run: u64) {
        self.pending
            .lock()
            .await
            .retain(|_, entry| entry.run != run);
    }

    /// Resolves a pending question with what the user answered. An empty list is
    /// a dismissal. Returns `false` when the id is unknown (a duplicate
    /// response, or a request that already timed out).
    pub async fn resolve(&self, id: u64, answers: Vec<Answer>) -> bool {
        let Some(pending) = self.pending.lock().await.remove(&id) else {
            return false;
        };
        pending.sender.send(answers).is_ok()
    }
}
