//! Electrobun entry point for the Oxide desktop app.
//!
//! The project/session/turn logic lives in the `oxide_desktop` library and the
//! shared `oxide-core`; this binary owns the window Electrobun opens and routes
//! every command the page sends to `commands::dispatch`. It is built by
//! Electrobun's own build — the manifest and binary `electrobun.config.ts`
//! names, with no feature to turn on (see `Cargo.toml`) — so the package is a
//! project of its own rather than a workspace member.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod approval;
mod ask;
mod bridge;
mod commands;

use bridge::{EventSink, Host};
use commands::{dispatch, DesktopState};
use electrobun::{
    self, Core, Rect, WebviewCallbacks, WebviewOptions, WindowCallbacks, WindowOptions,
};
use oxide_desktop::manager::DesktopManager;
use serde_json::Value;
use std::ffi::{c_char, CStr};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

/// The window's two ids are assigned by Electrobun, so the callbacks it hands
/// back run before there is a `Host` to name them with: the core and the state
/// are kept here, where an `extern "C"` frame can reach them.
static CORE: OnceLock<&'static Core> = OnceLock::new();
static STATE: OnceLock<Arc<DesktopState>> = OnceLock::new();

/// A callback runs on whichever thread Electrobun called back on rather than
/// inside a task, so the runtime is reached by its handle instead of through an
/// entered context: a request is answered on a task of this runtime either way.
static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();

/// The id of the app menu's update item, which [`menu_clicked`] compares against
/// and which the menu's own JSON carries as that item's action.
#[cfg(target_os = "macos")]
const CHECK_FOR_UPDATES: &str = "check-for-updates";

/// Shared with the webview's preload, which derives the key it encrypts the
/// internal channel with from it. This app speaks the host channel instead, so
/// the value only has to be a key rather than a secret.
const SECRET_KEY: &str =
    "1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32";

fn main() {
    if let Err(error) = run() {
        eprintln!("[oxide] {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    // A turn streams on its own task and an approval or a question is answered
    // later still, so the command layer spawns from whichever thread Electrobun
    // calls back on. The runtime stays alive for the life of the process and is
    // reached by its handle.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let _entered = runtime.enter();
    RUNTIME
        .set(runtime.handle().clone())
        .map_err(|_| "the runtime was already running".to_string())?;

    // The core lives as long as the app does: it is the handle every callback
    // and every command is given, and it is dropped only when the process ends.
    let core: &'static Core = Box::leak(Box::new(Core::load()?));
    let bundle_paths = electrobun::resolve_bundle_paths()?;
    let app_info = electrobun::resolve_app_info_from_bundle(&bundle_paths)?;
    CORE.set(core)
        .map_err(|_| "the Electrobun core was already loaded".to_string())?;

    // macOS keeps a menu bar open whatever the app is doing; Windows and Linux
    // show one only because an app asked for it, so the app does not gain a menu
    // bar for this one item — the sidebar's own button is the way in there.
    // The menu is set once the window is up (see [`create_window`]): the native
    // side builds `NSMenu` against `NSApp`, which exists only after
    // [`Core::run_main_thread`] has initialized the application.

    // The window has to be created after the event loop is running, so it is
    // built on a thread of its own.
    let handle = runtime.handle().clone();
    let _ui_thread = thread::spawn(move || create_window(core, &bundle_paths, handle));

    // Blocks on the native event loop; must run on the main thread.
    core.run_main_thread(&app_info)
}

/// Opens the one window the app is, shows the page inside it, and starts the
/// launch's own update check.
fn create_window(
    core: &'static Core,
    bundle_paths: &electrobun::BundlePaths,
    runtime: tokio::runtime::Handle,
) {
    thread::sleep(Duration::from_millis(150));

    if let Err(error) = core.configure_webview_runtime_from_executable_dir(bundle_paths, 0) {
        eprintln!("[oxide] failed to configure the webview runtime: {error}");
        return;
    }

    let mut window = WindowOptions::new("Oxide", Rect::new(0.0, 0.0, 1200.0, 800.0));
    window.centered = true;
    window.callbacks = WindowCallbacks {
        close: Some(window_closed),
        resize: Some(window_resized),
        ..WindowCallbacks::default()
    };
    let window_id = match core.create_window(window) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("[oxide] failed to create the window: {error}");
            return;
        }
    };

    let mut webview = WebviewOptions::new(
        window_id,
        "views://main/index.html",
        Rect::new(0.0, 0.0, 1200.0, 800.0),
    );
    webview.secret_key = SECRET_KEY;
    // The page is the app's own script, loaded from the app's own protocol, and
    // it speaks the bridge the main process answers on. The Electron-style
    // default in the SDK's type layer is the one for an app's own view; the
    // sandboxed preload is for content a host does not own, and it installs no
    // channel back into the page.
    webview.sandbox = false;
    webview.spell_check = true;
    // The page's own packets arrive on the user bridge, which the core queues
    // for the process that owns the window — a Rust main process has no socket
    // server to read them from, so [`drain_host_messages`] takes them off that
    // queue. The event channel is wired to the same dispatcher as the fallback
    // the page uses where a webview has no user bridge at all.
    webview.callbacks = WebviewCallbacks {
        decide_navigation: Some(decide_navigation),
        event: Some(webview_event),
        event_bridge: Some(event_bridge_message),
        ..WebviewCallbacks::default()
    };
    let webview_id = match core.create_webview(webview) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("[oxide] failed to create the webview: {error}");
            let _ = core.close_window(window_id);
            return;
        }
    };

    // The window navigates to the page it was given and nothing else, which the
    // native side enforces from the policy kept on the webview rather than from
    // the callback (see [`NAVIGATION_RULES`]).
    if let Err(error) = core.set_webview_navigation_rules(webview_id, NAVIGATION_RULES) {
        eprintln!("[oxide] failed to restrict the window's navigation: {error}");
    }

    let host = Arc::new(Host::new(core, webview_id));
    let state = Arc::new(DesktopState::new(
        DesktopManager::load_lossy(),
        EventSink::new(host),
    ));
    if STATE.set(Arc::clone(&state)).is_err() {
        eprintln!("[oxide] the window was already built");
        return;
    }

    // With the window up, the app is running and its `NSApplication` exists, so
    // the menu bar can be built — and a click on it has a window to reach, since
    // the state it answers through is set first. Electrobun's native side calls
    // `objc_setAssociatedObject(NSApp, …)` over the menu it parses, which is a
    // null dereference before the application is initialized.
    #[cfg(target_os = "macos")]
    if let Err(error) = core.set_application_menu_json(&menu_json(), Some(menu_clicked)) {
        eprintln!("[oxide] failed to set the application menu: {error}");
    }

    spawn_host_message_drain(core);

    // The app keeps itself current the way its other front-ends do: the newest
    // release of its own train is looked for in the background and, where this
    // copy is one the app replaces in place, installed without being asked. What
    // a launch is left to say is that a restart will run it, which is the
    // window's own row — and every step is kept beside the state as well as
    // emitted, since the page subscribes to the event channel after the install
    // has started.
    runtime.spawn(commands::auto_update(state));
}

/// The app is one window, and a run belongs to it: closing the window ends the
/// process on every platform rather than leaving a turn nothing can watch,
/// answer or stop. macOS keeps a windowless app alive by default, and a dock
/// icon that reopens nothing is not a second way back into the project.
extern "C" fn window_closed(_window_id: u32) {
    if let Some(core) = CORE.get() {
        let _ = core.stop_event_loop();
    }
}

/// The page is the app's own document, and the one thing the window may
/// navigate to. A link in a reply is opened in the machine's browser by the host
/// command the page calls, so a navigation to one would be a second way out of
/// the app — or a remote document painted in the window that is the app. The
/// rules are Electrobun's own: each is matched against the whole URL with `*`
/// alone as its wildcard and the last match deciding, so the first rule refuses
/// every scheme and the second lets the app's own view through.
const NAVIGATION_RULES: &str = r#"["^*","views://main/*"]"#;

extern "C" fn decide_navigation(_webview_id: u32, url: *const c_char) -> u32 {
    u32::from(cstr(url).starts_with("views://main/"))
}

/// The smallest window the layout is built for — the floor the shell this
/// replaced kept as its own window option. Electrobun has no minimum size to
/// set, so it is kept where a resize arrives: a window reported below it is
/// asked for the minimum instead. The two frame calls speak in the window's
/// outer size while the resize reports the room the page is laid out in, so the
/// chrome between them is measured from the window as it is rather than assumed;
/// the resize this answers reports a page already at the minimum, so the clamp
/// settles instead of asking again.
const MIN_WINDOW: (f64, f64) = (820.0, 560.0);

/// A layout rounds its own numbers, so a size within half a point of the floor
/// is the floor.
const FLOOR_SLACK: f64 = 0.5;

extern "C" fn window_resized(window_id: u32, _x: f64, _y: f64, width: f64, height: f64) {
    let Some(core) = CORE.get() else {
        return;
    };
    if width >= MIN_WINDOW.0 - FLOOR_SLACK && height >= MIN_WINDOW.1 - FLOOR_SLACK {
        return;
    }
    let chrome = core
        .get_window_frame(window_id)
        .map(|frame| (frame.width - width, frame.height - height))
        .unwrap_or((0.0, 0.0));
    let _ = core.set_window_size(
        window_id,
        MIN_WINDOW.0 + chrome.0.max(0.0),
        MIN_WINDOW.1 + chrome.1.max(0.0),
    );
}

/// The user bridge Electrobun queues for the main process. Nothing else reads
/// that queue, so it is drained on a thread of its own: a message posted while
/// the app is busy with a turn would otherwise wait for the next one, and the
/// page would sit on a request nothing had picked up. The queue is a channel
/// rather than a condition variable, so the wait between messages is a short
/// sleep — the interval Electrobun's own Rust templates use.
fn spawn_host_message_drain(core: &'static Core) {
    thread::spawn(move || loop {
        let mut drained = false;
        while let Some((_webview_id, message)) = core.pop_next_queued_host_message_string() {
            handle_host_message(&message);
            drained = true;
        }
        if !drained {
            thread::sleep(Duration::from_millis(10));
        }
    });
}

/// The event channel's fallback, for a page that had no user bridge to post on:
/// the preload sends it as a `host-message` event, wrapped in the envelope every
/// webview event travels in, and this unwraps it back to the packet the page
/// wrote. Every other event Electrobun reports this way — a load, a navigation —
/// is not the page talking, so only the one name is read.
extern "C" fn event_bridge_message(_webview_id: u32, message: *const c_char) {
    let Ok(envelope) = serde_json::from_str::<Value>(&cstr(message)) else {
        return;
    };
    if envelope["id"] != "webviewEvent" || envelope["type"] != "message" {
        return;
    }
    if envelope["payload"]["eventName"] != "host-message" {
        return;
    }
    if let Some(packet) = envelope["payload"]["detail"].as_str() {
        handle_host_message(packet);
    }
}

/// The same event, reported by name rather than wrapped — the shape a webview
/// event takes when it arrives over the internal bridge.
extern "C" fn webview_event(_webview_id: u32, event_name: *const c_char, detail: *const c_char) {
    if cstr(event_name) == "host-message" {
        handle_host_message(&cstr(detail));
    }
}

/// Everything the page says arrives here. A request is answered by the command
/// layer, and the caller is Electrobun's own thread or the drain thread, so the
/// work is handed to the runtime rather than done in that frame.
fn handle_host_message(message: &str) {
    let Some(state) = STATE.get() else {
        return;
    };
    let Ok(packet) = serde_json::from_str::<Value>(message) else {
        return;
    };
    if packet["type"] != "request" {
        return;
    }
    let id = packet["id"].as_u64().unwrap_or_default();
    let command = packet["params"]["command"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let args = packet["params"]["args"].clone();

    let state = Arc::clone(state);
    let Some(runtime) = RUNTIME.get() else {
        return;
    };
    runtime.spawn(async move {
        let host = state.host();
        let answer = dispatch(Arc::clone(&state), &host, &command, args).await;
        let _ = host.respond(id, answer);
    });
}

/// The menu bar's own click handler. Electrobun hands it the item's id and the
/// action the menu's JSON gave it, and the one item this app adds is the update
/// check.
#[cfg(target_os = "macos")]
extern "C" fn menu_clicked(_id: u32, action: *const c_char) {
    if !cstr(action).ends_with(CHECK_FOR_UPDATES) {
        return;
    }
    let Some(state) = STATE.get() else {
        return;
    };
    // The window performs the check and paints what it found, so the menu item
    // and the sidebar button end at one dialog.
    commands::announce_check_updates(&state.host());
}

/// The menu bar this app builds, with **Check for Updates…** in the app menu
/// where a macOS user looks for it (directly under **About**). The labels are
/// written out because this is the JSON Electrobun's native side builds `NSMenu`
/// from, rather than the TypeScript layer that fills them in from the role.
#[cfg(target_os = "macos")]
fn menu_json() -> String {
    use serde_json::json;

    // Every item is spelled `enabled: true`, the top-level ones included. The
    // TypeScript SDK fills the field in as it serializes a menu, but this entry
    // point is handed raw JSON and the native side reads a missing `enabled` as
    // `false`: a menu built without it is drawn with its items greyed out and
    // nothing to click — and a disabled top-level item takes its whole submenu
    // with it, so every command under it stops answering.
    json!([
        {
            "label": "Oxide",
            "enabled": true,
            "submenu": [
                { "label": "About Oxide", "role": "about", "enabled": true },
                { "type": "divider" },
                { "label": "Check for Updates…", "action": CHECK_FOR_UPDATES, "enabled": true },
                { "type": "divider" },
                { "label": "Hide Oxide", "role": "hide", "enabled": true },
                { "label": "Hide Others", "role": "hideOthers", "enabled": true },
                { "label": "Show All", "role": "showAll", "enabled": true },
                { "type": "divider" },
                { "label": "Quit Oxide", "role": "quit", "enabled": true },
            ],
        },
        {
            "label": "Edit",
            "enabled": true,
            "submenu": [
                { "label": "Undo", "role": "undo", "enabled": true },
                { "label": "Redo", "role": "redo", "enabled": true },
                { "type": "divider" },
                { "label": "Cut", "role": "cut", "enabled": true },
                { "label": "Copy", "role": "copy", "enabled": true },
                { "label": "Paste", "role": "paste", "enabled": true },
                { "label": "Select All", "role": "selectAll", "enabled": true },
            ],
        },
        {
            "label": "Window",
            "enabled": true,
            "submenu": [
                { "label": "Minimize", "role": "minimize", "enabled": true },
                { "label": "Zoom", "role": "zoom", "enabled": true },
                { "type": "divider" },
                { "label": "Close", "role": "close", "enabled": true },
            ],
        },
    ])
    .to_string()
}

/// Reads a C string Electrobun handed to a callback.
fn cstr(value: *const c_char) -> String {
    if value.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned()
}
