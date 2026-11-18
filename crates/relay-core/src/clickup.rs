//! ClickUp import client (PLAN 3.6, `link`/`import` only). Reads one list; Relay never
//! pushes titles, descriptions or comments back. The personal token goes in the
//! `Authorization` header **without** a `Bearer` prefix, as ClickUp documents.

use serde_json::Value;

pub const DEFAULT_BASE_URL: &str = "https://api.clickup.com/api/v2";
const TIMEOUT_SECS: u64 = 20;

#[derive(Debug, thiserror::Error)]
pub enum ClickUpError {
    #[error("clickup request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("clickup answered {status}: {message}")]
    Status { status: u16, message: String },
    #[error("clickup response is missing {0}")]
    Missing(&'static str),
    #[error("clickup json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing clickup token")]
    MissingToken,
}

/// One task as the import sees it. `due_date` is Unix milliseconds; `priority` is 1-4
/// (1 urgent, 4 low).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClickUpTask {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub status: Option<String>,
    pub due_date: Option<i64>,
    pub priority: Option<i64>,
    pub url: Option<String>,
}

pub struct ClickUpClient {
    token: String,
    http: reqwest::Client,
    base: String,
}

impl std::fmt::Debug for ClickUpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClickUpClient")
            .field("token", &"<redacted>")
            .field("base", &self.base)
            .finish()
    }
}

impl ClickUpClient {
    pub fn new(token: String) -> Result<Self, ClickUpError> {
        Self::with_base(token, DEFAULT_BASE_URL.to_string())
    }

    pub fn with_base(token: String, base: String) -> Result<Self, ClickUpError> {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(ClickUpError::MissingToken);
        }
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .build()?;
        Ok(Self {
            token,
            http,
            base: base.trim_end_matches('/').to_string(),
        })
    }

    /// A cheap authenticated call: the workspace's spaces.
    pub async fn check(&self) -> Result<(), ClickUpError> {
        self.get("/team").await.map(|_| ())
    }

    /// The first page (up to 100) of a list's tasks.
    pub async fn list_tasks(
        &self,
        list_id: &str,
        include_closed: bool,
    ) -> Result<Vec<ClickUpTask>, ClickUpError> {
        let mut out = Vec::new();
        for page in 0..10 {
            let path = format!(
                "/list/{list_id}/task?include_closed={include_closed}&subtasks=false&page={page}"
            );
            let value = self.get(&path).await?;
            let tasks = value
                .get("tasks")
                .and_then(Value::as_array)
                .ok_or(ClickUpError::Missing("tasks"))?;
            out.extend(tasks.iter().filter_map(task_from));
            if tasks.len() < 100 {
                break;
            }
        }
        Ok(out)
    }

    async fn get(&self, path: &str) -> Result<Value, ClickUpError> {
        let response = self
            .http
            .get(format!("{}{}", self.base, path))
            .header("Authorization", &self.token)
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            return Err(ClickUpError::Status { status, message });
        }
        Ok(response.json().await?)
    }
}

fn task_from(value: &Value) -> Option<ClickUpTask> {
    let id = value.get("id").and_then(Value::as_str)?.to_string();
    let name = value.get("name").and_then(Value::as_str)?.to_string();
    let due_date = match value.get("due_date") {
        Some(Value::String(text)) => text.parse().ok(),
        Some(Value::Number(number)) => number.as_i64(),
        _ => None,
    };
    let priority = value
        .get("priority")
        .and_then(|priority| priority.get("id"))
        .and_then(|id| match id {
            Value::String(text) => text.parse().ok(),
            Value::Number(number) => number.as_i64(),
            _ => None,
        });
    Some(ClickUpTask {
        id,
        name,
        description: value
            .get("description")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        status: value
            .pointer("/status/status")
            .and_then(Value::as_str)
            .map(str::to_string),
        due_date,
        priority,
        url: value.get("url").and_then(Value::as_str).map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        http::{HeaderMap, header},
        response::Json,
        routing::get,
    };
    use serde_json::json;

    use super::*;

    async fn mock() -> String {
        let app = Router::new()
            .route(
                "/list/L1/task",
                get(|headers: HeaderMap| async move {
                    use axum::response::IntoResponse;
                    let authorization = headers
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default();
                    // ClickUp expects the raw token, not a Bearer prefix.
                    if authorization != "pk_test" {
                        return (axum::http::StatusCode::UNAUTHORIZED, "bad token").into_response();
                    }
                    Json(json!({
                        "tasks": [
                            {
                                "id": "abc",
                                "name": "Ship the RC",
                                "description": "Check the installer",
                                "status": {"status": "in progress"},
                                "due_date": "1795000000000",
                                "priority": {"id": "2"},
                                "url": "https://app.clickup.com/t/abc"
                            },
                            {"name": "no id is skipped"}
                        ]
                    }))
                    .into_response()
                }),
            )
            .route("/team", get(|| async { Json(json!({"teams": []})) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn list_tasks_maps_the_fields_and_sends_the_raw_token() {
        let base = mock().await;
        let client = ClickUpClient::with_base("pk_test".into(), base).unwrap();
        let tasks = client.list_tasks("L1", false).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "abc");
        assert_eq!(tasks[0].name, "Ship the RC");
        assert_eq!(tasks[0].status.as_deref(), Some("in progress"));
        assert_eq!(tasks[0].due_date, Some(1_795_000_000_000));
        assert_eq!(tasks[0].priority, Some(2));
        assert_eq!(
            tasks[0].url.as_deref(),
            Some("https://app.clickup.com/t/abc")
        );
        assert!(client.check().await.is_ok());
    }

    #[test]
    fn an_empty_token_is_refused_before_any_request() {
        assert!(matches!(
            ClickUpClient::new("  ".into()),
            Err(ClickUpError::MissingToken)
        ));
    }
}
