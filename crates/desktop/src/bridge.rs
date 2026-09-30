//! Transport shared by the Electron host and the desktop command layer.
//!
//! The renderer never talks to this process directly. Electron's sandboxed
//! preload exposes a narrow command API, the main process forwards those calls
//! as JSON lines on stdin, and this host writes responses and streamed events
//! as JSON lines on stdout.

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct EventSink {
    output: mpsc::UnboundedSender<Value>,
}

impl EventSink {
    pub fn new(output: mpsc::UnboundedSender<Value>) -> Self {
        Self { output }
    }

    pub fn emit(&self, event: &str, payload: impl Serialize) -> Result<(), String> {
        let payload = serde_json::to_value(payload).map_err(|error| error.to_string())?;
        self.output
            .send(json!({
                "type": "event",
                "event": event,
                "payload": payload,
            }))
            .map_err(|_| "desktop event channel is closed".to_string())
    }

    pub fn response(&self, id: u64, result: Result<Value, String>) {
        let frame = match result {
            Ok(value) => json!({ "type": "response", "id": id, "ok": true, "value": value }),
            Err(error) => json!({ "type": "response", "id": id, "ok": false, "error": error }),
        };
        let _ = self.output.send(frame);
    }
}
