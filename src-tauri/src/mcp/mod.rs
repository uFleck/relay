//! The MCP adapter: one loopback HTTP server with the agent endpoint (`/mcp`) and the analyst endpoint
//! (`/mcp/analyst`), the two bearer tokens, and the status the setup sheet reads through `mcp_status`.

mod analyst;
mod auth;
mod server;
mod tools;

#[cfg(test)]
mod tests;

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use relay_core::store::Store;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

pub use server::{DEFAULT_PORT, Tokens};

const AGENT_TOKEN_FILE: &str = "mcp-agent.token";
const ANALYST_TOKEN_FILE: &str = "mcp-analyst.token";
const STATUS_EVENT: &str = "mcp://status";

/// What the setup sheet shows. The tokens are local bearer tokens for this machine's loopback server (the
/// sheet copies them into the Executor/OpenCode config), not third-party secrets.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub running: bool,
    pub port: u16,
    pub url: String,
    pub analyst_url: String,
    pub error: Option<String>,
    pub agent_token: Option<String>,
    pub analyst_token: Option<String>,
}

/// Server handle and the last status, kept in Tauri state. Dropping it stops the listener.
#[derive(Default)]
pub struct McpState {
    status: Mutex<McpStatus>,
    running: Mutex<Option<server::Running>>,
}

impl McpState {
    pub fn status(&self) -> McpStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn set(&self, app: &AppHandle, status: McpStatus) {
        if let Ok(mut s) = self.status.lock() {
            *s = status.clone();
        }
        let _ = app.emit(STATUS_EVENT, status);
    }
}

pub fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

pub fn analyst_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp/analyst")
}

/// The reviewer agent's system prompt; the headless runner writes it into OpenCode's config.
pub fn analyst_instructions() -> &'static str {
    analyst::INSTRUCTIONS
}

/// Reads a token file, or writes a fresh 128-bit random token the first time (or if the file is unusable).
fn load_token(path: &Path) -> std::io::Result<String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let existing = existing.trim();
        if existing.len() >= 32 && existing.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(existing.to_owned());
        }
    }
    let token = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(path, &token)?;
    Ok(token)
}

pub fn load_tokens(dir: &Path) -> Result<Tokens, String> {
    let read = |name: &str| {
        load_token(&dir.join(name)).map_err(|e| format!("could not read or create {name}: {e}"))
    };
    Ok(Tokens {
        agent: read(AGENT_TOKEN_FILE)?,
        analyst: read(ANALYST_TOKEN_FILE)?,
    })
}

/// Starts the server in the background on [`DEFAULT_PORT`] and records the outcome (also as `mcp://status`).
/// A taken port is reported as an error; the server never moves to another port.
pub fn start(app: AppHandle, store: Arc<Store>, dir: &Path) {
    let base = McpStatus {
        port: DEFAULT_PORT,
        url: url(DEFAULT_PORT),
        analyst_url: analyst_url(DEFAULT_PORT),
        ..McpStatus::default()
    };
    let tokens = match load_tokens(dir) {
        Ok(tokens) => tokens,
        Err(error) => {
            app.state::<McpState>().set(
                &app,
                McpStatus {
                    error: Some(error),
                    ..base
                },
            );
            return;
        }
    };
    tauri::async_runtime::spawn(async move {
        let status = McpStatus {
            agent_token: Some(tokens.agent.clone()),
            analyst_token: Some(tokens.analyst.clone()),
            ..base
        };
        let state = app.state::<McpState>();
        match server::serve(store, tokens, DEFAULT_PORT).await {
            Ok(running) => {
                if let Ok(mut r) = state.running.lock() {
                    *r = Some(running);
                }
                state.set(
                    &app,
                    McpStatus {
                        running: true,
                        ..status
                    },
                );
            }
            Err(error) => {
                eprintln!("relay: MCP server did not start: {error}");
                state.set(
                    &app,
                    McpStatus {
                        error: Some(error),
                        ..status
                    },
                );
            }
        }
    });
}

#[tauri::command]
pub fn mcp_status(state: tauri::State<'_, McpState>) -> McpStatus {
    state.status()
}
