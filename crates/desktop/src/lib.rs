//! Desktop front-end for Oxide.
//!
//! `manager` holds the multi-project and session-listing logic, `turn` runs an
//! agent turn against a project using the shared `oxide-core` configuration,
//! `at` answers the composer's `@path` completion, `git` names the repository the
//! composer's top row shows, `repository` reads uncommitted changes, `pulls`
//! reads the pull requests a thread reported, `prefs` holds the window's own
//! preferences, `search` finds a phrase across the stored threads, and `update`
//! checks the app's own release train and installs it. `terminal` is the pseudoterminal a
//! conversation can open. These build without any GUI dependency so they
//! are unit tested like the rest of the workspace; the window in `electron/` is
//! what turns them into packets, and the engine in `src/main.rs` is what hands it
//! each one.

pub mod at;
pub mod git;
pub mod manager;
pub mod prefs;
pub mod pulls;
pub mod repository;
pub mod search;
pub mod staging;
pub mod terminal;
pub mod turn;
pub mod update;

pub use at::{AtAnswer, PathCache};
pub use git::GitView;
pub use manager::{
    expand_project_path, load_project_config, load_project_config_with, set_project_trust,
    DesktopManager, Project, ProjectRegistry, ProjectTrust, ProjectView, SessionView,
};
pub use prefs::Prefs;
pub use pulls::{PullStore, PullView};
pub use turn::{open_session, start_turn, Turn};
