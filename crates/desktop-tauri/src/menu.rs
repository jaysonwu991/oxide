//! The macOS menu Electron builds, expressed with Tauri's native menu API.
//!
//! Menu actions that belong to the page are announced on the same
//! `oxide:to-page` event as engine packets. Menu labels and accelerators follow
//! the language and shortcut maps the page sends, so both desktop windows keep
//! one keyboard contract.

use serde_json::{json, Value};
use std::collections::HashMap;
use tauri::menu::{Menu, MenuEvent, MenuItem, MenuItemBuilder, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
pub struct NativeMenu {
    strings: HashMap<String, String>,
    pub bindings: HashMap<String, String>,
}

impl NativeMenu {
    pub fn with_strings(strings: HashMap<String, String>) -> Self {
        Self {
            strings,
            bindings: HashMap::new(),
        }
    }

    pub fn text(&self, key: &str, fallback: &str) -> String {
        self.strings
            .get(key)
            .cloned()
            .unwrap_or_else(|| fallback.to_string())
    }

    fn accelerator(&self, action: &str, fallback: &str) -> String {
        let Some(chord) = self.bindings.get(action) else {
            return fallback.to_string();
        };
        let chord = chord.to_ascii_lowercase();
        let mut parts: Vec<&str> = chord.split('+').collect();
        let Some(key) = parts.pop() else {
            return fallback.to_string();
        };
        if key.is_empty() {
            return fallback.to_string();
        }
        let mut tokens = Vec::new();
        for part in parts {
            tokens.push(match part {
                "mod" => "CmdOrCtrl",
                "ctrl" | "control" => "Ctrl",
                "shift" => "Shift",
                "alt" | "opt" | "option" => "Alt",
                _ => return fallback.to_string(),
            });
        }
        tokens.push(match key {
            "escape" => "Esc",
            "tab" => "Tab",
            "space" => "Space",
            "plus" | "=" => "Plus",
            key if key.len() == 1 => {
                return format!("{}+{}", tokens.join("+"), key.to_ascii_uppercase());
            }
            _ => return fallback.to_string(),
        });
        tokens.join("+")
    }
}

pub fn install(app: &AppHandle, state: &NativeMenu) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        app.remove_menu().map_err(|error| error.to_string())?;
        return Ok(());
    }

    let about =
        PredefinedMenuItem::about(app, Some(&state.text("ui.aboutOxide", "About Oxide")), None)
            .map_err(|error| error.to_string())?;
    let updates = item(
        app,
        "check-updates",
        &state.text("ui.checkForUpdates", "Check for Updates…"),
        None,
    )?;
    let settings = item(
        app,
        "open-settings",
        &state.text("settings.title", "Settings…"),
        Some(&state.accelerator("settings", "CmdOrCtrl+,")),
    )?;
    let hide = PredefinedMenuItem::hide(app, Some(&state.text("ui.hideOxide", "Hide Oxide")))
        .map_err(|error| error.to_string())?;
    let hide_others =
        PredefinedMenuItem::hide_others(app, Some(&state.text("ui.hideOthers", "Hide Others")))
            .map_err(|error| error.to_string())?;
    let show_all = PredefinedMenuItem::show_all(app, Some(&state.text("ui.showAll", "Show All")))
        .map_err(|error| error.to_string())?;
    let quit = PredefinedMenuItem::quit(app, Some(&state.text("ui.quitOxide", "Quit Oxide")))
        .map_err(|error| error.to_string())?;
    let app_menu = Submenu::with_items(
        app,
        "Oxide",
        true,
        &[
            &about,
            &separator(app)?,
            &updates,
            &settings,
            &separator(app)?,
            &hide,
            &hide_others,
            &show_all,
            &separator(app)?,
            &quit,
        ],
    )
    .map_err(|error| error.to_string())?;

    let new_tab = item(
        app,
        "tab-new",
        &state.text("ui.newTab", "New Tab"),
        Some("CmdOrCtrl+T"),
    )?;
    let close_tab = item(
        app,
        "tab-close",
        &state.text("ui.closeTab", "Close Tab"),
        Some("CmdOrCtrl+W"),
    )?;
    let file_menu = Submenu::with_items(
        app,
        state.text("ui.file", "File"),
        true,
        &[&new_tab, &close_tab],
    )
    .map_err(|error| error.to_string())?;

    let undo = PredefinedMenuItem::undo(app, Some(&state.text("ui.undo", "Undo")))
        .map_err(|error| error.to_string())?;
    let redo = PredefinedMenuItem::redo(app, Some(&state.text("ui.redo", "Redo")))
        .map_err(|error| error.to_string())?;
    let cut = PredefinedMenuItem::cut(app, Some(&state.text("ui.cut", "Cut")))
        .map_err(|error| error.to_string())?;
    let copy = PredefinedMenuItem::copy(app, Some(&state.text("ui.copy", "Copy")))
        .map_err(|error| error.to_string())?;
    let paste = PredefinedMenuItem::paste(app, Some(&state.text("ui.paste", "Paste")))
        .map_err(|error| error.to_string())?;
    let select_all =
        PredefinedMenuItem::select_all(app, Some(&state.text("ui.selectAll", "Select All")))
            .map_err(|error| error.to_string())?;
    let edit_menu = Submenu::with_items(
        app,
        state.text("composer.edit", "Edit"),
        true,
        &[
            &undo,
            &redo,
            &separator(app)?,
            &cut,
            &copy,
            &paste,
            &select_all,
        ],
    )
    .map_err(|error| error.to_string())?;

    let command_menu = item(
        app,
        "command-menu",
        &state.text("settings.actionCommandMenu", "Command Menu…"),
        Some(&state.accelerator("command-menu", "CmdOrCtrl+K")),
    )?;
    let larger = item(
        app,
        "zoom-in",
        &state.text("settings.actionLargerText", "Larger Text"),
        Some(&state.accelerator("font-larger", "CmdOrCtrl+Plus")),
    )?;
    let smaller = item(
        app,
        "zoom-out",
        &state.text("settings.actionSmallerText", "Smaller Text"),
        Some(&state.accelerator("font-smaller", "CmdOrCtrl+-")),
    )?;
    let reset = item(
        app,
        "zoom-reset",
        &state.text("settings.actionResetText", "Actual Size"),
        Some(&state.accelerator("font-reset", "CmdOrCtrl+0")),
    )?;
    let panel = item(
        app,
        "toggle-panel",
        &state.text("settings.actionTogglePanel", "Show or Hide Panel"),
        Some(&state.accelerator("toggle-panel", "CmdOrCtrl+J")),
    )?;
    let terminal = item(
        app,
        "open-terminal",
        &state.text("settings.actionTerminal", "Terminal"),
        Some(&state.accelerator("terminal", "Ctrl+`")),
    )?;
    let pull_requests = item(
        app,
        "open-pulls",
        &state.text("menu.pulls", "Pull requests…"),
        None,
    )?;
    let sidebar = item(
        app,
        "toggle-sidebar",
        &state.text("settings.actionToggleSidebar", "Show or Hide Sidebar"),
        Some(&state.accelerator("toggle-sidebar", "CmdOrCtrl+B")),
    )?;
    let back = item(
        app,
        "history-back",
        &state.text("settings.actionBack", "Back"),
        Some(&state.accelerator("back", "CmdOrCtrl+[")),
    )?;
    let forward = item(
        app,
        "history-forward",
        &state.text("settings.actionForward", "Forward"),
        Some(&state.accelerator("forward", "CmdOrCtrl+]")),
    )?;
    let fullscreen = PredefinedMenuItem::fullscreen(
        app,
        Some(&state.text("ui.toggleFullScreen", "Toggle Full Screen")),
    )
    .map_err(|error| error.to_string())?;
    let view_menu = Submenu::with_items(
        app,
        state.text("ui.view", "View"),
        true,
        &[
            &command_menu,
            &separator(app)?,
            &larger,
            &smaller,
            &reset,
            &separator(app)?,
            &panel,
            &terminal,
            &pull_requests,
            &separator(app)?,
            &sidebar,
            &back,
            &forward,
            &separator(app)?,
            &fullscreen,
        ],
    )
    .map_err(|error| error.to_string())?;

    let minimize = PredefinedMenuItem::minimize(app, Some(&state.text("ui.minimize", "Minimize")))
        .map_err(|error| error.to_string())?;
    let maximize = PredefinedMenuItem::maximize(app, Some(&state.text("ui.zoom", "Zoom")))
        .map_err(|error| error.to_string())?;
    let close_window = item(
        app,
        "close-window",
        &state.text("ui.closeWindow", "Close Window"),
        Some("CmdOrCtrl+Shift+W"),
    )?;
    let window_menu = Submenu::with_items(
        app,
        state.text("ui.window", "Window"),
        true,
        &[&minimize, &maximize, &separator(app)?, &close_window],
    )
    .map_err(|error| error.to_string())?;

    let menu = Menu::with_items(
        app,
        &[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu],
    )
    .map_err(|error| error.to_string())?;
    app.set_menu(menu).map_err(|error| error.to_string())?;
    Ok(())
}

fn item(
    app: &AppHandle,
    id: &str,
    label: &str,
    accelerator: Option<&str>,
) -> Result<MenuItem<tauri::Wry>, String> {
    let mut builder = MenuItemBuilder::with_id(id, label);
    if let Some(accelerator) = accelerator {
        builder = builder.accelerator(accelerator);
    }
    builder.build(app).map_err(|error| error.to_string())
}

fn separator(app: &AppHandle) -> Result<PredefinedMenuItem<tauri::Wry>, String> {
    PredefinedMenuItem::separator(app).map_err(|error| error.to_string())
}

pub fn handle(app: &AppHandle, event: &MenuEvent) {
    let id = event.id().as_ref();
    let (event, payload) = match id {
        "check-updates" => ("check-updates", json!({})),
        "open-settings" => ("open-settings", json!({})),
        "command-menu" => ("command-menu", json!({})),
        "zoom-in" => ("zoom", json!({ "action": "in" })),
        "zoom-out" => ("zoom", json!({ "action": "out" })),
        "zoom-reset" => ("zoom", json!({ "action": "reset" })),
        "toggle-panel" => ("toggle-panel", json!({})),
        "open-terminal" => ("open-terminal", json!({})),
        "open-pulls" => ("open-pulls", json!({})),
        "toggle-sidebar" => ("toggle-sidebar", json!({})),
        "history-back" => ("history", json!({ "action": "back" })),
        "history-forward" => ("history", json!({ "action": "forward" })),
        "tab-new" => ("chat-tab", json!({ "action": "new" })),
        "tab-close" => ("chat-tab", json!({ "action": "close" })),
        "close-window" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.close();
            }
            return;
        }
        _ => return,
    };
    announce(app, event, payload);
}

fn announce(app: &AppHandle, event: &str, payload: Value) {
    let packet = json!({ "type": "message", "id": event, "payload": payload }).to_string();
    let _ = app.emit("oxide:to-page", packet);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_chord_becomes_the_platform_accelerator() {
        let menu = NativeMenu {
            strings: HashMap::new(),
            bindings: HashMap::from([("settings".to_string(), "mod+shift+s".to_string())]),
        };
        assert_eq!(
            menu.accelerator("settings", "CmdOrCtrl+,"),
            "CmdOrCtrl+Shift+S"
        );
        assert_eq!(menu.accelerator("missing", "CmdOrCtrl+B"), "CmdOrCtrl+B");
    }

    #[test]
    fn labels_come_from_the_shared_language_catalog() {
        let menu = NativeMenu {
            strings: HashMap::from([("ui.newTab".to_string(), "新标签页".to_string())]),
            bindings: HashMap::new(),
        };
        assert_eq!(menu.text("ui.newTab", "New Tab"), "新标签页");
        assert_eq!(menu.text("missing", "Fallback"), "Fallback");
    }
}
