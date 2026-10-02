//! Where a desktop event leaves the run that produced it.
//!
//! A turn streams on its own task, and an approval or a question is answered
//! later still, so the sink the command layer, the approval broker and the ask
//! broker share is a handle to the window itself: `commands::dispatch` answers
//! the command the page invoked, and everything the run has to say while one is
//! in flight is emitted on the app's own event channel, where the page's
//! `listen` handlers pick it up.

use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Clone)]
pub struct EventSink {
    app: AppHandle,
}

impl EventSink {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    /// Announces an event to the window. A payload that will not serialize, or
    /// a window that is gone, is reported to the caller and otherwise ignored:
    /// a run whose event cannot be painted still has to finish.
    pub fn emit(&self, event: &str, payload: impl Serialize) -> Result<(), String> {
        let payload = serde_json::to_value(payload).map_err(|error| error.to_string())?;
        self.app
            .emit(event, payload)
            .map_err(|error| error.to_string())
    }
}
