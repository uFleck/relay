//! Relay core: pure Rust, no Tauri. `cargo test -p relay-core` is the fast loop.
//!
//! The local SQLite v2 database is the source of truth. Adapters (Tauri IPC, headless
//! Claude Slack transport, GitHub, ClickUp) stay thin; policy lives here. The retired v1
//! automatic intake pipeline (JEV, episodes, analysis jobs, proposals, app-token Slack)
//! and its database are gone with no migration path.

pub mod claude_slack;
pub mod clickup;
pub mod clickup_sync;
pub mod git;
pub mod github;
pub mod model;
pub mod provider;
pub mod repos;
pub mod t3_dispatch;
pub mod t3_progress;
pub mod task_lifecycle;
pub mod v2;
