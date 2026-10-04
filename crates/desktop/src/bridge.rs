//! The window the app is, and where a desktop event leaves the run that produced
//! it.
//!
//! An Electrobun app's window belongs to the main process, which is this one: a
//! turn streams on its own task and an approval or a question is answered later
//! still, so the sink the command layer, the approval broker and the ask broker
//! share is a handle to the window itself — the Electrobun core, the window it
//! opened and the webview inside it.
//!
//! Both directions carry one JSON packet. The page posts a request on the user
//! bridge — `window.__electrobunHostBridge.postMessage(...)`, which the core
//! queues for the main process and `src/main.rs` drains — and the host answers or
//! announces through [`Core::send_host_message_to_webview_json`], which the page
//! receives in `window.__electrobun.receiveMessageFromHost`. A packet is a
//! request (`{type, id, method, params}`), its answer
//! (`{type, id, success, payload|error}`), or an announcement (`{type, id,
//! payload}`, whose `id` is the event's own name) — the envelope Electrobun's
//! own preload bridge uses, so the page needs no bundler and no generated
//! schema, and a front-end that is not the desktop's can speak it too.

use electrobun::{Core, OpenFileDialogOptions};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// The window the app is: the Electrobun core and the webview shown inside the
/// window it opened.
pub struct Host {
    core: &'static Core,
    webview_id: u32,
}

impl Host {
    pub fn new(core: &'static Core, webview_id: u32) -> Self {
        Self { core, webview_id }
    }

    /// Announces an event to the window. A payload that will not serialize, or a
    /// page that is not listening yet, is reported to the caller and otherwise
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

    fn send(&self, packet: &Value) -> Result<(), String> {
        self.core
            .send_host_message_to_webview_json(self.webview_id, &packet.to_string())
            .map_err(|error| error.to_string())
    }

    /// Opens the platform's own folder chooser. Used by the desktop's **Add**
    /// button when the path field is empty, so adding a project does not require
    /// typing an absolute path from memory.
    ///
    /// The panel is the app's own (`NSOpenPanel`, GTK's chooser, the Windows
    /// equivalent), not a chooser shelled out to `osascript`/`zenity`: a child
    /// process's panel opens as a background app, which can put it behind the
    /// window — or never show it at all where the platform refuses the request —
    /// and the user is left with a button that appears to do nothing.
    ///
    /// The call blocks until the panel closes, which the runtime's blocking pool
    /// is for: the page's own task goes on being able to answer the rest of the
    /// window while the user is choosing.
    pub async fn pick_folder(&self) -> Result<Option<String>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let core = self.core;
        tokio::task::spawn_blocking(move || {
            let chosen = core.open_file_dialog(OpenFileDialogOptions {
                starting_folder: "",
                allowed_file_types: "",
                can_choose_files: false,
                can_choose_directory: true,
                allows_multiple_selection: false,
            });
            let _ = tx.send(chosen);
        })
        .await
        .map_err(|error| error.to_string())?;
        // The panel answers with a JSON array of paths and an empty string for a
        // dismissed one.
        let chosen = rx.await.map_err(|error| error.to_string())??;
        let paths: Vec<String> = serde_json::from_str(&chosen).unwrap_or_default();
        Ok(paths.into_iter().next())
    }

    /// Opens an external link in the machine's own browser. The transcript
    /// renders URLs as anchors, but the webview cannot navigate to a remote page,
    /// so a click is routed here instead of relying on `target="_blank"`.
    ///
    /// The launch is Electrobun's: the platform's own call hands the URL to
    /// whatever the machine uses as its handler. A URL is that handler's own
    /// argument, never text a shell reads.
    pub fn open_url(&self, url: &str) -> Result<(), String> {
        let url = url.trim();
        if !is_openable_url(url) {
            return Err("Only http(s) links can be opened".to_string());
        }
        match self.core.open_external(url) {
            Ok(true) => Ok(()),
            Ok(false) => Err("The system could not open that link".to_string()),
            Err(error) => Err(error),
        }
    }

    /// Runs the app again, which is what runs a release an install has put in
    /// place: the process running is still the build that started, so only a new
    /// one is the new version.
    ///
    /// Electrobun calls nothing into the app to relaunch it, so this is the
    /// machine's own way of starting an app — the bundle handed to LaunchServices
    /// on macOS (with `-n`, so a second copy really starts rather than the running
    /// one being raised) and the launcher this process was started from
    /// elsewhere — and the event loop is stopped once it is on its way, since the
    /// caller is the copy being replaced.
    pub fn restart(&self) -> Result<(), String> {
        restart_self()?;
        let _ = self.core.stop_event_loop();
        Ok(())
    }
}

fn restart_self() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(err)?;
    #[cfg(target_os = "macos")]
    if let Some(bundle) = bundle_of(&executable) {
        std::process::Command::new("open")
            .arg("-n")
            .arg(bundle)
            .spawn()
            .map_err(err)?;
        return Ok(());
    }
    std::process::Command::new(&executable)
        .args(std::env::args_os().skip(1))
        .spawn()
        .map_err(err)?;
    Ok(())
}

/// The `.app` a bundle's own executable sits in, if it is one: a macOS app is
/// launched from `<bundle>/Contents/MacOS/<launcher>`.
#[cfg(target_os = "macos")]
fn bundle_of(executable: &std::path::Path) -> Option<&std::path::Path> {
    let macos = executable.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    if bundle.extension()? != "app" {
        return None;
    }
    Some(bundle)
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn is_openable_url(url: &str) -> bool {
    let scheme = url.to_ascii_lowercase();
    scheme.starts_with("https://") || scheme.starts_with("http://")
}

#[derive(Clone)]
pub struct EventSink {
    host: Arc<Host>,
}

impl EventSink {
    pub fn new(host: Arc<Host>) -> Self {
        Self { host }
    }

    /// The window this event leaves from, for a command that is the app's own
    /// rather than the project's — opening the folder chooser, restarting.
    pub fn host(&self) -> Arc<Host> {
        Arc::clone(&self.host)
    }

    /// Announces an event to the window. A payload that will not serialize, or a
    /// window that is gone, is reported to the caller and otherwise ignored: a
    /// run whose event cannot be painted still has to finish.
    pub fn emit(&self, event: &str, payload: impl Serialize) -> Result<(), String> {
        self.host.emit(event, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_links_are_openable() {
        assert!(is_openable_url("https://github.com/o/r/pull/7"));
        assert!(is_openable_url("http://localhost:3000"));
        assert!(is_openable_url("HTTPS://example.com"));
        assert!(!is_openable_url("file:///etc/passwd"));
        assert!(!is_openable_url("javascript:alert(1)"));
        assert!(!is_openable_url(""));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_bundle_is_found_above_its_own_executable() {
        let bundle = bundle_of(std::path::Path::new(
            "/Applications/Oxide.app/Contents/MacOS/launcher",
        ));
        assert_eq!(
            bundle,
            Some(std::path::Path::new("/Applications/Oxide.app"))
        );

        assert_eq!(
            bundle_of(std::path::Path::new("/tmp/target/debug/oxide-desktop")),
            None
        );
        assert_eq!(
            bundle_of(std::path::Path::new(
                "/Applications/Oxide.app/Contents/Resources/x"
            )),
            None
        );
    }
}
