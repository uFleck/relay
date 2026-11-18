//! Hand-rolled MCP over Streamable HTTP, copied from Hopper's shape: `POST /mcp` for working agents and
//! `POST /mcp/analyst` for the reviewer, JSON responses only, loopback only, 1 MB bodies. Each endpoint has
//! its own bearer token and its own tool table; the endpoint, not the caller, decides which tools exist.

use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::post,
};
use relay_core::store::Store;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{
    analyst,
    auth::{self, Gate},
    tools::{self, ToolError},
};

pub const DEFAULT_PORT: u16 = 38547;
const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
pub const MAX_BODY: usize = 1024 * 1024;

/// The two bearer tokens. Never logged, never put in an error message.
#[derive(Clone)]
pub struct Tokens {
    pub agent: String,
    pub analyst: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    Agent,
    Analyst,
}

#[derive(Clone)]
struct Ctx {
    store: Arc<Store>,
    endpoint: Endpoint,
}

/// A running server. Dropping it (or calling `stop`) shuts the listener down.
pub struct Running {
    stop: Option<oneshot::Sender<()>>,
}

impl Running {
    pub fn stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Both endpoints, each behind its own gate. `port` is the one the `Host` header must name.
pub fn router(store: Arc<Store>, tokens: Tokens, port: u16) -> Router {
    let endpoint = |path: &str, endpoint: Endpoint, token: String| {
        Router::new()
            .route(path, post(rpc).get(not_allowed).delete(not_allowed))
            .route_layer(middleware::from_fn_with_state(
                Gate { token, port },
                auth::gate,
            ))
            .with_state(Ctx {
                store: store.clone(),
                endpoint,
            })
    };
    Router::new()
        .merge(endpoint("/mcp", Endpoint::Agent, tokens.agent))
        .merge(endpoint("/mcp/analyst", Endpoint::Analyst, tokens.analyst))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

/// Binds exactly `127.0.0.1:port` and serves until the returned handle is dropped. A taken port is an error,
/// never a silent move: client configs name this port.
pub async fn serve(store: Arc<Store>, tokens: Tokens, port: u16) -> Result<Running, String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied => {
                format!("port {port} is taken by another process; free it and restart Relay ({e})")
            }
            _ => format!("could not listen on 127.0.0.1:{port} ({e})"),
        })?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let app = router(store, tokens, addr.port());
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await;
    });
    Ok(Running { stop: Some(stop) })
}

async fn not_allowed() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response()
}

fn json_response(value: Value) -> Response {
    (
        // Windows PowerShell decodes a body without a charset as Latin-1, which garbles accents.
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        value.to_string(),
    )
        .into_response()
}

fn failure(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

async fn rpc(State(ctx): State<Ctx>, body: Bytes) -> Response {
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return json_response(failure(Value::Null, -32700, "parse error"));
    };
    let reply = match message {
        Value::Array(items) => {
            let mut replies = Vec::new();
            for item in items {
                replies.extend(handle(&ctx, item).await);
            }
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        single => handle(&ctx, single).await,
    };
    reply.map_or_else(|| StatusCode::ACCEPTED.into_response(), json_response)
}

async fn handle(ctx: &Ctx, message: Value) -> Option<Value> {
    let method = message.get("method")?.as_str()?.to_owned();
    // A message without an id is a notification: no reply.
    let id = message.get("id")?.clone();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method.as_str() {
        "initialize" => Ok(initialize(ctx.endpoint, &params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": match ctx.endpoint {
            Endpoint::Agent => tools::definitions(),
            Endpoint::Analyst => analyst::definitions(),
        }})),
        "tools/call" => call_tool(ctx, &params).await,
        other => Err((-32601, format!("method not found: {other}"))),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => failure(id, code, &message),
    })
}

fn initialize(endpoint: Endpoint, params: &Value) -> Value {
    let asked = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("");
    let version = SUPPORTED
        .iter()
        .find(|v| **v == asked)
        .unwrap_or(&SUPPORTED[0]);
    let (name, instructions) = match endpoint {
        Endpoint::Agent => ("relay", tools::INSTRUCTIONS),
        Endpoint::Analyst => ("relay-analyst", analyst::INSTRUCTIONS),
    };
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": name, "version": env!("CARGO_PKG_VERSION")},
        "instructions": instructions,
    })
}

fn tool_result(text: String, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

async fn call_tool(ctx: &Ctx, params: &Value) -> Result<Value, (i32, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    if ctx.endpoint == Endpoint::Analyst && !analyst::ALLOWED.contains(&name.as_str()) {
        let message = if tools::NAMES.contains(&name.as_str()) {
            format!("tool not allowed on the analyst endpoint: {name}")
        } else {
            format!("unknown tool: {name}")
        };
        return Err((-32602, message));
    }
    // The store is synchronous (one SQLite writer behind a mutex), so calls run off the async workers.
    let store = ctx.store.clone();
    let endpoint = ctx.endpoint;
    let tool = name.clone();
    let outcome = tokio::task::spawn_blocking(move || match endpoint {
        Endpoint::Agent => tools::call(&store, &tool, args),
        Endpoint::Analyst => analyst::call(&store, &tool, args),
    })
    .await
    .unwrap_or_else(|_| Some(Err(ToolError::new("internal", "tool call panicked"))));
    match outcome {
        None => Err((-32602, format!("unknown tool: {name}"))),
        Some(Ok(value)) => Ok(tool_result(value.to_string(), false)),
        Some(Err(e)) => Ok(tool_result(e.to_json(), true)),
    }
}
