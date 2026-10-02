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
#[cfg(target_os = "macos")]
use commands::announce_check_updates;
use commands::{dispatch, DesktopState};
use oxide_desktop::manager::DesktopManager;
use serde_json::Value;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

/// The id of the app menu's update item, which `on_menu_event` compares against.
#[cfg(target_os = "macos")]
const CHECK_FOR_UPDATES: &str = "check-for-updates";

/// Tauri's default menu with **Check for Updates…** added to the app menu, where
/// a macOS user looks for it (directly under **About**).
///
/// macOS keeps a menu bar open whatever the app is doing; Windows and Linux show
/// one only because an app asked for it, so the app does not gain a menu bar for
/// this one item — the sidebar's own button is the way in there.
#[cfg(target_os = "macos")]
fn menu(handle: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind};

    let menu = Menu::default(handle)?;
    let item = MenuItem::with_id(
        handle,
        CHECK_FOR_UPDATES,
        "Check for Updates…",
        true,
        None::<&str>,
    )?;
    // The app submenu leads the menu bar and is the only place this may go: the
    // top level holds submenus alone on macOS.
    if let Some(MenuItemKind::Submenu(app)) = menu.items()?.into_iter().next() {
        // About, its separator, then this.
        app.insert_items(&[&item], 2)?;
    }
    Ok(menu)
}

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
    let builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu).on_menu_event(|handle, event| {
        if event.id() == CHECK_FOR_UPDATES {
            // The window performs the check and paints what it found, so the
            // menu item and the sidebar button end at one dialog.
            announce_check_updates(handle);
        }
    });

    let app = builder
        .setup(|app| {
            let events = EventSink::new(app.handle().clone());
            app.manage(Arc::new(DesktopState::new(
                DesktopManager::load_lossy(),
                events,
            )));
            // The app keeps itself current the way its other front-ends do: the
            // newest release of its own train is looked for in the background
            // and, where this copy is one the app replaces in place, installed
            // without being asked. What a launch is left to say is that a restart
            // will run it, which is the window's own row — and every step is kept
            // beside the state as well as emitted, since the page subscribes to
            // the event channel after the install has started.
            let state = app.state::<Arc<DesktopState>>().inner().clone();
            tauri::async_runtime::spawn(commands::auto_update(state));
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
