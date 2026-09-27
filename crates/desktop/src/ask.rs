//! Interactive questions for the desktop.
//!
//! When the model follows a skill that asks something only the user can answer,
//! it calls the `ask` tool and the agent awaits this broker. The broker emits a
//! `question-request` event carrying a request id and the questions, and the UI
//! answers with the `resolve_question` command: the values the user picked or
//! typed, or nothing at all for a dismissed dialog. A request that never gets an
//! answer times out as a dismissal so a turn cannot hang forever.

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
/// answered.
const MAX_PENDING: usize = 32;

struct Pending {
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

    /// An `Asker` for the agent runtime that routes each question here.
    pub fn asker(self: &Arc<Self>) -> Asker {
        let broker = Arc::clone(self);
        Arc::new(move |questions| {
            let broker = Arc::clone(&broker);
            Box::pin(async move { broker.request(questions).await })
        })
    }

    async fn request(&self, questions: Vec<Question>) -> Option<Reply> {
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
            pending.insert(id, Pending { sender: tx });
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
            _ => {
                self.pending.lock().await.remove(&id);
                None
            }
        }
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
