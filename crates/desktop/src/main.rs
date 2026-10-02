//! Tauri entry point for the Oxide desktop app.
//!
//! The project/session/turn logic lives in the `oxide_desktop` library and the
//! shared `oxide-core`; this binary wires that state to one window and forwards
//! every command the window invokes to `commands::dispatch`. It builds with
//! `--features gui` (see `cargo run -p oxide-desktop --features gui`).

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod approval;
mod ask;
mod bridge;
mod commands;

use bridge::EventSink;
use commands::{dispatch, DesktopState};
use oxide_desktop::manager::DesktopManager;
use serde_json::Value;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

/// The one command the window invokes: the name and arguments of a command in
/// [`commands::dispatch`], answered with that command's own value or the reason
/// it failed.
///
/// The webview reaches its own app through the Tauri globals rather than an
/// allowlisted bridge, so the alternative is one `#[tauri::command]` per name
/// and a typed argument list for each. Keeping the single entry point leaves the
/// set of commands the window can perform in one match, where the argument
/// shapes and the two operating-system-only calls (`pick_folder`, `open_url`)
/// sit beside the state they act on, and the page's own bridge stays the four
/// lines of `ui/app.js` that unpack this shape.
#[tauri::command]
async fn oxide_invoke(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    command: String,
    args: Value,
) -> Result<Value, String> {
    dispatch(Arc::clone(&state), &app, &command, args).await
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let events = EventSink::new(app.handle().clone());
            app.manage(Arc::new(DesktopState::new(
                DesktopManager::load_lossy(),
                events,
            )));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![oxide_invoke])
        .build(tauri::generate_context!())
        .expect("error while building the Oxide desktop app");

    app.run(|handle, event| {
        // The app is one window, and a run belongs to it: closing the window
        // ends the process on every platform rather than leaving a turn
        // nothing can watch, answer or stop. macOS keeps a windowless app
        // alive by default, and a dock icon that reopens nothing is not a
        // second way back into the project.
        if let tauri::RunEvent::WindowEvent {
            event: tauri::WindowEvent::CloseRequested { .. },
            ..
        } = event
        {
            handle.exit(0);
        }
    });
}
