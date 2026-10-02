//! Desktop front-end for Oxide.
//!
//! `manager` holds the multi-project and session-listing logic, `turn` runs an
//! agent turn against a project using the shared `oxide-core` configuration, and
//! `at` answers the composer's `@path` completion. All three build without any
//! GUI dependency so they are unit tested like the rest of the workspace; the
//! Tauri shell in `src/main.rs` is the only part that links a window, and it
//! drives these three.

pub mod at;
pub mod manager;
pub mod turn;

pub use at::{AtAnswer, PathCache};
pub use manager::{
    expand_project_path, load_project_config, load_project_config_with, set_project_trust,
    DesktopManager, Project, ProjectRegistry, ProjectTrust, ProjectView,
};
pub use turn::{open_session, start_turn, Turn};
