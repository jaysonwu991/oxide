//! Shared agent core for the oxide CLI and desktop app.
//!
//! Everything the agent needs to run — configuration, providers, tools, MCP,
//! sessions, snapshots, plugins and the agent loop — lives here so the terminal
//! CLI and the desktop front-end share one implementation and the same
//! on-disk configuration.

pub mod agent;
pub mod auth;
pub mod cli;
pub mod clipboard;
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
pub mod tools;
pub mod trust;
