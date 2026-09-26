//! Interactive tool approval for the desktop.
//!
//! When a permission rule resolves to `ask`, the agent awaits this broker. The
//! broker emits an `approval-request` event carrying a request id, and the UI
//! answers with the `resolve_approval` command (`deny`, `once`, or `always`).
//! `always` records a per-project rule in the shared
//! [`oxide_core::approvals::ApprovalStore`] so the prompt does not repeat for
//! that tool in the terminal, the extension or here. A request that never gets
//! an answer times out as a denial so a turn cannot hang forever.

use oxide_core::agent::Approver;
use oxide_core::approvals::ApprovalStore;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::{oneshot, Mutex};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

struct Pending {
    sender: oneshot::Sender<bool>,
    project: PathBuf,
    tool: String,
}

pub struct ApprovalBroker {
    app: AppHandle,
    pending: Mutex<HashMap<u64, Pending>>,
    store: Mutex<ApprovalStore>,
    next: AtomicU64,
}

impl ApprovalBroker {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            pending: Mutex::new(HashMap::new()),
            store: Mutex::new(ApprovalStore::load()),
            next: AtomicU64::new(1),
        }
    }

    /// An `Approver` for the agent runtime that routes each request here. The
    /// project scopes "always allow" rules.
    pub fn approver(self: &Arc<Self>, project: PathBuf) -> Approver {
        let broker = Arc::clone(self);
        Arc::new(move |tool, detail| {
            let broker = Arc::clone(&broker);
            let project = project.clone();
            Box::pin(async move { broker.request(project, tool, detail).await })
        })
    }

    async fn request(&self, project: PathBuf, tool: String, detail: String) -> bool {
        if self.store.lock().await.is_allowed(&project, &tool) {
            return true;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(
            id,
            Pending {
                sender: tx,
                project,
                tool: tool.clone(),
            },
        );
        let _ = self.app.emit(
            "approval-request",
            json!({ "id": id, "tool": tool, "detail": detail }),
        );
        match tokio::time::timeout(APPROVAL_TIMEOUT, rx).await {
            Ok(Ok(approved)) => approved,
            _ => {
                self.pending.lock().await.remove(&id);
                false
            }
        }
    }

    /// Resolves a pending request. `decision` is `deny`, `once`, or `always`.
    /// Returns `false` when the id is unknown (a duplicate response, or a
    /// request that already timed out).
    pub async fn resolve(&self, id: u64, decision: &str) -> bool {
        let Some(pending) = self.pending.lock().await.remove(&id) else {
            return false;
        };
        let approved = decision != "deny";
        if approved && decision == "always" {
            if let Err(err) = self
                .store
                .lock()
                .await
                .allow(&pending.project, &pending.tool)
            {
                let _ = self.app.emit(
                    "agent-event",
                    json!({ "type": "error", "message": format!("could not save approval rule: {err}") }),
                );
            }
        }
        pending.sender.send(approved).is_ok()
    }

    pub async fn list(&self, project: &Path) -> Vec<String> {
        self.store.lock().await.list(project)
    }

    pub async fn clear(&self, project: &Path) -> Result<(), String> {
        self.store
            .lock()
            .await
            .clear(project)
            .map_err(|err| err.to_string())
    }
}
