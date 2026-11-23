//! The packet channel between the desktop page and the engine.
//!
//! The Tauri host owns the screen, the menu, the folder chooser and the release
//! it was installed from; the engine owns the projects, sessions and turns. One
//! JSON packet per line is the whole contract, so every rule about a run stays
//! in the engine.
//!
//! A packet is a request (`{type, id, method, params}`), its answer (`{type,
//! id, success, payload|error}}`), or an announcement (`{type, id, payload}`,
//! whose `id` is the event's own name). The host forwards both directions
//! unchanged.
//!
//! The host reads packets from this sink rather than the process stdout.

use serde::Serialize;
use serde_json::{json, Value};
use std::io::Write;
use std::sync::{Arc, Mutex};

/// The window's side of the channel: the writer every answer and every
/// announcement leaves through.
pub struct Host {
    out: Mutex<Box<dyn Write + Send>>,
}

impl Host {
    pub fn new(out: impl Write + Send + 'static) -> Self {
        Self {
            out: Mutex::new(Box::new(out)),
        }
    }

    /// A channel that writes packets to stdout.
    pub fn stdout() -> Self {
        Self::new(std::io::stdout())
    }

    /// Announces an event to the window. A payload that will not serialize, or
    /// a window that has gone away, is reported to the caller and otherwise
    /// ignored: a run whose event cannot be painted still has to finish.
    pub fn emit(&self, event: &str, payload: impl Serialize) -> Result<(), String> {
        let payload = serde_json::to_value(payload).map_err(|error| error.to_string())?;
        self.send(&json!({ "type": "message", "id": event, "payload": payload }))
    }

    /// Answers one request from the page.
    pub fn respond(&self, id: u64, answer: Result<Value, String>) -> Result<(), String> {
        let packet = match answer {
            Ok(payload) => {
                json!({ "type": "response", "id": id, "success": true, "payload": payload })
            }
            Err(error) => json!({ "type": "response", "id": id, "success": false, "error": error }),
        };
        self.send(&packet)
    }

    /// Writes one packet as one line, and flushes it: the window reads this
    /// pipe as it arrives, so a packet left in a buffer is a request the page
    /// waits on.
    fn send(&self, packet: &Value) -> Result<(), String> {
        let mut out = self
            .out
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        writeln!(out, "{packet}")
            .and_then(|()| out.flush())
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
pub struct EventSink {
    host: Arc<Host>,
}

impl EventSink {
    pub fn new(host: Arc<Host>) -> Self {
        Self { host }
    }

    /// Announces an event to the window. A payload that will not serialize, or
    /// a window that is gone, is reported to the caller and otherwise ignored:
    /// a run whose event cannot be painted still has to finish.
    pub fn emit(&self, event: &str, payload: impl Serialize) -> Result<(), String> {
        self.host.emit(event, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A writer a test can read back: what the window's pipe would have
    /// received.
    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn lines(&self) -> Vec<Value> {
            let bytes = self.0.lock().unwrap().clone();
            let text = String::from_utf8(bytes).unwrap();
            text.lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }

    impl Write for SharedBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn host() -> (Host, SharedBuffer) {
        let buffer = SharedBuffer::default();
        (Host::new(buffer.clone()), buffer)
    }

    #[test]
    fn an_event_is_one_line_naming_the_event() {
        let (host, buffer) = host();
        host.emit("agent-event", json!({ "kind": "text" })).unwrap();
        assert_eq!(
            buffer.lines(),
            vec![json!({
                "type": "message",
                "id": "agent-event",
                "payload": { "kind": "text" },
            })]
        );
    }

    #[test]
    fn an_answer_carries_its_own_id_and_whether_it_worked() {
        let (host, buffer) = host();
        host.respond(7, Ok(json!(["a"]))).unwrap();
        host.respond(8, Err("no".to_string())).unwrap();
        assert_eq!(
            buffer.lines(),
            vec![
                json!({ "type": "response", "id": 7, "success": true, "payload": ["a"] }),
                json!({ "type": "response", "id": 8, "success": false, "error": "no" }),
            ]
        );
    }
}
