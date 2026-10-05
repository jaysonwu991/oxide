//! Shared agent core for the oxide CLI and desktop app.
//!
//! Everything the agent needs to run — configuration, providers, tools, MCP,
//! sessions, snapshots, plugins and the agent loop — lives here so the terminal
//! CLI and the desktop front-end share one implementation and the same
//! on-disk configuration.

pub mod agent;
pub mod approval;
pub mod approvals;
pub mod ask;
pub mod at;
pub mod auth;
pub mod changes;
pub(crate) mod child;
pub mod cli;
pub mod clipboard;
pub mod commands;
pub mod compact;
pub mod config;
pub mod diff;
pub mod ecosystem;
pub mod html;
pub mod llm;
pub mod lsp;
pub mod mcp;
pub mod mcp_config;
pub mod mcp_oauth;
pub mod media;
pub mod memory;
pub mod notice;
pub mod notify;
pub mod permission;
pub mod plugin;
pub mod plugin_registry;
pub mod portkey_usage;
pub mod pricing;
pub mod runner;
pub mod session;
pub mod sessions;
pub mod snapshots;
pub mod theme_view;
pub mod title;
pub mod tools;
pub mod trust;
pub mod update_notice;
pub mod updates;
pub mod workspaces;

/// A test that points a provider at a stub host writes the process environment,
/// which every other test in the binary can see. One such test runs at a time.
#[cfg(test)]
pub(crate) mod env_lock {
    use std::sync::atomic::{AtomicBool, Ordering};

    static BUSY: AtomicBool = AtomicBool::new(false);

    pub(crate) struct Held(());

    /// Waits until no other holder has the environment, and keeps it until the
    /// returned guard is dropped. A spin rather than a mutex because the same
    /// guard has to be held by a test that awaits and by one that does not.
    /// Sleeping between attempts keeps a waiter from starving the holder.
    pub(crate) fn hold() -> Held {
        while BUSY.swap(true, Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Held(())
    }

    impl Drop for Held {
        fn drop(&mut self) {
            BUSY.store(false, Ordering::SeqCst);
        }
    }
}
