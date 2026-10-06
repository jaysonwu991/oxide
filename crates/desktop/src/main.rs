//! The harness behind the Oxide desktop app.
//!
//! The app itself is the Electron window in `crates/desktop/electron`: it owns
//! the screen, the menu bar, the folder chooser, the app's own release and the
//! relaunch that runs one. This process is what that window starts, and the
//! project/session/turn logic lives here and in the shared `oxide-core`, behind
//! the one JSON packet per line channel `src/bridge.rs` describes.
//!
//! Requests arrive on stdin — the window forwards the page's own
//! `oxide_invoke` packets unchanged — and every answer and announcement leaves
//! on stdout. A request is answered on a task of its own, so a turn that
//! streams for minutes does not stop the window from cancelling it or steering
//! it; the process ends when its stdin does, which is the window quitting or
//! being killed.

mod approval;
mod ask;
mod bridge;
mod commands;

use bridge::{EventSink, Host};
use commands::{dispatch, DesktopState};
use oxide_desktop::manager::DesktopManager;
use serde_json::Value;
use std::io::BufRead;
use std::sync::Arc;

fn main() {
    if let Err(error) = run() {
        eprintln!("[oxide] {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    // A turn streams on its own task and an approval or a question is answered
    // later still, so every request is spawned onto this runtime. It stays
    // alive for the life of the process.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let _entered = runtime.enter();

    let host = Arc::new(Host::stdout());
    let state = Arc::new(DesktopState::new(
        DesktopManager::load_lossy(),
        EventSink::new(Arc::clone(&host)),
    ));

    // The app keeps itself current the way its other front-ends do: the newest
    // release of its own train is looked for in the background and, where this
    // copy is one the app replaces in place, installed without being asked.
    // What a launch is left to say is that a restart will run it, which is the
    // window's own row — and every step is kept beside the state as well as
    // emitted, since the page subscribes to the channel after the install has
    // started.
    runtime.spawn(commands::auto_update(Arc::clone(&state)));
    // The context window a model resolves to comes from the catalog its provider
    // published, so the launch looks for a newer one beside the update check:
    // without it a model released since this build keeps the built-in table's
    // conservative window for the life of the session.
    runtime.spawn(commands::auto_catalog(Arc::clone(&state)));

    // The window owns this process's lifetime: it spawned it, so its stdin
    // closing is the app quitting.
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let Some((id, command, args)) = request(&line) else {
            continue;
        };
        let state = Arc::clone(&state);
        let host = Arc::clone(&host);
        runtime.spawn(async move {
            let answer = dispatch(Arc::clone(&state), &host, &command, args).await;
            let _ = host.respond(id, answer);
        });
    }
    Ok(())
}

/// One line of the page's own packet, as the command it asks for: the request's
/// id, the command's name and its arguments. Anything else on this pipe — a
/// blank line, a packet of another type, a line that is not JSON — is not a
/// request and is left alone rather than answered, so a client that speaks
/// another shape to the same window cannot be mistaken for the page.
fn request(line: &str) -> Option<(u64, String, Value)> {
    let packet: Value = serde_json::from_str(line).ok()?;
    if packet["type"] != "request" {
        return None;
    }
    let id = packet["id"].as_u64()?;
    let command = packet["params"]["command"].as_str()?.to_string();
    Some((id, command, packet["params"]["args"].clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_request_is_read_off_the_wire() {
        let line = json!({
            "type": "request",
            "id": 4,
            "method": "oxide_invoke",
            "params": { "command": "list_projects", "args": {} },
        })
        .to_string();
        assert_eq!(
            request(&line),
            Some((4, "list_projects".to_string(), json!({})))
        );
    }

    #[test]
    fn anything_that_is_not_a_request_is_left_alone() {
        assert_eq!(request(""), None);
        assert_eq!(request("not json"), None);
        // The page's `init()` never writes these, but a client sharing the pipe
        // could: an announcement is not something to answer.
        assert_eq!(
            request(&json!({ "type": "message", "id": "agent-end" }).to_string()),
            None
        );
        assert_eq!(
            request(&json!({ "type": "request", "params": {} }).to_string()),
            None
        );
        assert_eq!(
            request(&json!({ "type": "request", "id": 1, "params": {} }).to_string()),
            None
        );
    }
}
