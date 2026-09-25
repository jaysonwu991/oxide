//! Tauri entry point for the Oxide desktop app.
//!
//! The project/session/turn logic lives in the `oxide_desktop` library and the
//! shared `oxide-core`; this binary only wires state and commands to a window.
//! It builds with `--features gui` (see `cargo run -p oxide-desktop --features gui`).

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod approval;
mod commands;

use commands::{
    add_project, all_sessions, cancel_run, clear_approvals, create_project, delete_session,
    list_approvals, list_models, list_projects, list_providers, list_sessions, list_themes, login,
    logout, open_url, pick_folder, project_info, remove_project, rename_session, resolve_approval,
    send_prompt, session_messages, set_model, set_project_trust, set_theme, steer_run,
    theme_colors, DesktopState,
};
use oxide_desktop::manager::DesktopManager;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let manager = DesktopManager::load().expect("loading desktop projects");
            app.manage(DesktopState::new(manager, app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_projects,
            add_project,
            create_project,
            pick_folder,
            remove_project,
            list_sessions,
            all_sessions,
            project_info,
            set_project_trust,
            session_messages,
            rename_session,
            delete_session,
            list_providers,
            login,
            logout,
            send_prompt,
            cancel_run,
            steer_run,
            resolve_approval,
            list_approvals,
            clear_approvals,
            list_models,
            set_model,
            list_themes,
            theme_colors,
            set_theme,
            open_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Oxide desktop app");
}
