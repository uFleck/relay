//! Relay core: pure Rust, no Tauri. `cargo test -p relay-core` is the fast loop.
//!
//! The local SQLite task graph is the source of truth (docs/PLAN.md section 2).
//! Adapters (Tauri IPC, MCP, Slack, JEV, GitHub, ClickUp) stay thin; policy lives here.

pub mod analysis;
pub mod candidates;
pub mod claude_slack;
pub mod clickup;
pub mod clickup_sync;
pub mod git;
pub mod github;
pub mod jev;
pub mod model;
pub mod provider;
pub mod repos;
pub mod slack;
pub mod store;
pub mod summaries;
pub mod t3_dispatch;
pub mod t3_progress;
pub mod v2;
pub mod view;
