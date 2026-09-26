//! Desktop front-end for Oxide.
//!
//! `manager` holds the multi-project and session-listing logic and `turn` runs
//! an agent turn against a project using the shared `oxide-core` configuration.
//! Both build without any GUI dependency so they are unit tested like the rest
//! of the workspace. The Tauri shell lives behind the `gui` feature.

pub mod manager;
pub mod turn;

pub use manager::{
    expand_project_path, load_project_config, load_project_config_with, set_project_trust,
    DesktopManager, Project, ProjectRegistry, ProjectTrust, ProjectView,
};
pub use turn::{open_session, start_turn, Turn};
