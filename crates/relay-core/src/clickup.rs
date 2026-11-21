//! ClickUp import client (PLAN 3.6, `link`/`import` only). Reads one list; Relay never
//! pushes titles, descriptions or comments back. The personal token goes in the
//! `Authorization` header **without** a `Bearer` prefix, as ClickUp documents.

use serde_json::Value;

use crate::provider::ProviderFailure;

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

impl ClickUpError {
    /// A safe typed outcome. 401 and a missing token are configuration failures; a 429 that
    /// survived the single retry still carries no raw body into the UI.
    pub fn failure(&self) -> ProviderFailure {
        match self {
            Self::Status { status, .. } => crate::provider::failure_from_status(*status, "", None),
            Self::MissingToken => ProviderFailure::Configuration,
            Self::Http(_) | Self::Missing(_) | Self::Json(_) => ProviderFailure::Unavailable,
        }
    }
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
    /// ClickUp's status type: `open`, `custom`, `done` or `closed`.
    pub status_type: Option<String>,
    pub list_name: Option<String>,
    pub due_date: Option<i64>,
    pub priority: Option<i64>,
    pub url: Option<String>,
    /// Provider update time as RFC 3339 UTC, when ClickUp supplied one. The Lens shows it as
    /// the card's last update; it is never used as the snapshot's local fetch time.
    pub date_updated: Option<String>,
    /// Assignee ids from the exact card read, for #31's live-state resolver.
    #[serde(default)]
    pub assignee_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Named {
    pub id: String,
    pub name: String,
}

/// Cards assigned to one user. `truncated` means the page cap was hit, so the list is
/// incomplete and must not be used to decide what is no longer assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignedTasks {
    pub tasks: Vec<ClickUpTask>,
    pub truncated: bool,
}

const PAGE_SIZE: usize = 100;
const MAX_PAGES: u32 = 20;

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

    /// One exact card read in one request. `GET /task/{native_id}` returns the complete card,
    /// so the picker never fans out a title search or chains an extra assignee call.
    pub async fn task(&self, native_id: &str) -> Result<ClickUpTask, ClickUpError> {
        let id = native_id.trim();
        if id.is_empty() {
            return Err(ClickUpError::Missing("task id"));
        }
        let value = self.get(&format!("/task/{}", encode_segment(id))).await?;
        task_from(&value).ok_or(ClickUpError::Missing("task"))
    }

    /// The first page (up to 100) of a list's tasks.
    pub async fn list_tasks(
        &self,
        list_id: &str,
        include_closed: bool,
    ) -> Result<Vec<ClickUpTask>, ClickUpError> {
        let mut out = Vec::new();
        let list_id = encode_segment(list_id);
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

    /// The authenticated user's id.
    pub async fn me(&self) -> Result<String, ClickUpError> {
        let value = self.get("/user").await?;
        id_string(value.pointer("/user/id")).ok_or(ClickUpError::Missing("user.id"))
    }

    pub async fn workspaces(&self) -> Result<Vec<Named>, ClickUpError> {
        let value = self.get("/team").await?;
        named_from(&value, "teams")
    }

    pub async fn spaces(&self, team_id: &str) -> Result<Vec<Named>, ClickUpError> {
        let path = format!("/team/{}/space?archived=false", encode_segment(team_id));
        named_from(&self.get(&path).await?, "spaces")
    }

    /// Open cards assigned to `user_id`, subtasks included, in one space or, without
    /// `space_id`, in the whole workspace. A guest sees no spaces from `spaces`, only
    /// "Shared with me" lists, so the workspace-wide query is the one that finds their cards.
    pub async fn assigned_tasks(
        &self,
        team_id: &str,
        user_id: &str,
        space_id: Option<&str>,
    ) -> Result<AssignedTasks, ClickUpError> {
        let team = encode_segment(team_id);
        let user = encode_segment(user_id);
        let space = space_id
            .map(|space| format!("&space_ids%5B%5D={}", encode_segment(space)))
            .unwrap_or_default();
        let mut tasks: Vec<ClickUpTask> = Vec::new();
        for page in 0..MAX_PAGES {
            let path = format!(
                "/team/{team}/task?assignees%5B%5D={user}{space}\
                 &subtasks=true&include_closed=false&page={page}"
            );
            let value = self.get(&path).await?;
            let raw = value
                .get("tasks")
                .and_then(Value::as_array)
                .ok_or(ClickUpError::Missing("tasks"))?;
            for task in raw.iter().filter_map(task_from) {
                if !tasks.iter().any(|seen| seen.id == task.id) {
                    tasks.push(task);
                }
            }
            let last = value.get("last_page").and_then(Value::as_bool) == Some(true);
            if last || raw.len() < PAGE_SIZE {
                return Ok(AssignedTasks {
                    tasks,
                    truncated: false,
                });
            }
        }
        Ok(AssignedTasks {
            tasks,
            truncated: true,
        })
    }

    async fn get(&self, path: &str) -> Result<Value, ClickUpError> {
        let mut response = self.send(path).await?;
        if response.status().as_u16() == 429 {
            // ClickUp documents Retry-After in seconds; honour one retry, capped.
            let wait = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(5)
                .min(30);
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            response = self.send(path).await?;
        }
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

    async fn send(&self, path: &str) -> Result<reqwest::Response, ClickUpError> {
        Ok(self
            .http
            .get(format!("{}{}", self.base, path))
            .header("Authorization", &self.token)
            .send()
            .await?)
    }
}

fn id_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn named_from(value: &Value, key: &'static str) -> Result<Vec<Named>, ClickUpError> {
    let items = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or(ClickUpError::Missing(key))?;
    Ok(items
        .iter()
        .filter_map(|item| {
            Some(Named {
                id: id_string(item.get("id"))?,
                name: item.get("name").and_then(Value::as_str)?.to_string(),
            })
        })
        .collect())
}

fn encode_segment(value: &str) -> String {
    let mut out = String::new();
    for byte in value.trim().bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
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
        status_type: value
            .pointer("/status/type")
            .and_then(Value::as_str)
            .map(str::to_string),
        list_name: value
            .pointer("/list/name")
            .and_then(Value::as_str)
            .map(str::to_string),
        due_date,
        priority,
        url: value.get("url").and_then(Value::as_str).map(str::to_string),
        date_updated: value
            .get("date_updated")
            .and_then(|raw| match raw {
                Value::String(text) => text.parse::<i64>().ok(),
                Value::Number(number) => number.as_i64(),
                _ => None,
            })
            .and_then(crate::model::from_unix_millis),
        assignee_ids: value
            .get("assignees")
            .and_then(Value::as_array)
            .map(|assignees| {
                assignees
                    .iter()
                    .filter_map(|user| id_string(user.get("id")))
                    .collect()
            })
            .unwrap_or_default(),
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

    async fn serve(app: Router) -> ClickUpClient {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        ClickUpClient::with_base("pk_test".into(), format!("http://{addr}")).unwrap()
    }

    #[tokio::test]
    async fn assigned_tasks_sends_assignee_and_space_and_pages_to_the_end() {
        use axum::extract::RawQuery;
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let app = Router::new()
            .route(
                "/user",
                get(|| async { Json(json!({"user": {"id": 42, "username": "me"}})) }),
            )
            .route(
                "/team",
                get(|| async { Json(json!({"teams": [{"id": "9", "name": "Umbler"}]})) }),
            )
            .route(
                "/team/9/space",
                get(|RawQuery(query): RawQuery| async move {
                    assert_eq!(query.as_deref(), Some("archived=false"));
                    Json(json!({"spaces": [{"id": "S1", "name": "Payment"}]}))
                }),
            )
            .route(
                "/team/9/task",
                get(move |RawQuery(query): RawQuery| {
                    let log = log.clone();
                    async move {
                        let query = query.unwrap_or_default();
                        let page = query.rsplit("page=").next().unwrap().to_string();
                        log.lock().unwrap().push(query);
                        let count = if page == "0" { 100 } else { 1 };
                        let tasks: Vec<_> = (0..count)
                            .map(|n| {
                                json!({
                                    "id": format!("{page}-{n}"),
                                    "name": "card",
                                    "status": {"status": "doing", "type": "custom"},
                                    "list": {"name": "Sprint"}
                                })
                            })
                            .collect();
                        Json(json!({"tasks": tasks, "last_page": page != "0"}))
                    }
                }),
            );
        let client = serve(app).await;

        assert_eq!(client.me().await.unwrap(), "42");
        let workspaces = client.workspaces().await.unwrap();
        assert_eq!(workspaces[0].name, "Umbler");
        assert_eq!(client.spaces("9").await.unwrap()[0].id, "S1");

        let assigned = client.assigned_tasks("9", "42", Some("S1")).await.unwrap();
        assert_eq!(assigned.tasks.len(), 101);
        assert!(!assigned.truncated);
        assert_eq!(assigned.tasks[0].status_type.as_deref(), Some("custom"));
        assert_eq!(assigned.tasks[0].list_name.as_deref(), Some("Sprint"));
        let with_space = seen.lock().unwrap().clone();
        assert_eq!(with_space.len(), 2);
        for query in &with_space {
            assert!(query.contains("assignees%5B%5D=42"), "{query}");
            assert!(query.contains("space_ids%5B%5D=S1"), "{query}");
            assert!(query.contains("subtasks=true"), "{query}");
            assert!(query.contains("include_closed=false"), "{query}");
        }

        seen.lock().unwrap().clear();
        client.assigned_tasks("9", "42", None).await.unwrap();
        let without_space = seen.lock().unwrap().clone();
        assert!(
            without_space[0].contains("assignees%5B%5D=42"),
            "{}",
            without_space[0]
        );
        assert!(
            !without_space[0].contains("space_ids"),
            "{}",
            without_space[0]
        );
    }

    #[tokio::test]
    async fn assigned_tasks_reports_truncation_at_the_page_cap() {
        let app = Router::new().route(
            "/team/9/task",
            get(
                |axum::extract::RawQuery(query): axum::extract::RawQuery| async move {
                    let page = query.unwrap().rsplit("page=").next().unwrap().to_string();
                    let tasks: Vec<_> = (0..100)
                        .map(|n| json!({"id": format!("{page}-{n}"), "name": "card"}))
                        .collect();
                    Json(json!({"tasks": tasks, "last_page": false}))
                },
            ),
        );
        let client = serve(app).await;
        let assigned = client.assigned_tasks("9", "42", None).await.unwrap();
        assert!(assigned.truncated);
        assert_eq!(assigned.tasks.len(), 2000);
    }

    #[test]
    fn an_empty_token_is_refused_before_any_request() {
        assert!(matches!(
            ClickUpClient::new("  ".into()),
            Err(ClickUpError::MissingToken)
        ));
    }

    #[tokio::test]
    async fn an_exact_card_read_is_one_request_with_assignees_and_update_time() {
        use axum::extract::RawQuery;
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = seen.clone();
        let app = Router::new().route(
            "/task/8x1K2",
            get(move |RawQuery(query): RawQuery| {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    assert!(query.is_none(), "the exact card read sends no query");
                    Json(json!({
                        "id": "8x1K2",
                        "name": "Ship the RC",
                        "status": {"status": "in progress", "type": "custom"},
                        "list": {"name": "Sprint"},
                        "date_updated": "1795000000000",
                        "assignees": [
                            {"id": 42, "username": "me"},
                            {"id": "77", "username": "partner"}
                        ],
                        "url": "https://app.clickup.com/t/8x1K2"
                    }))
                }
            }),
        );
        let client = serve(app).await;
        let task = client.task("8x1K2").await.unwrap();
        assert_eq!(task.id, "8x1K2");
        assert_eq!(task.name, "Ship the RC");
        assert_eq!(task.assignee_ids, vec!["42", "77"]);
        assert!(task.date_updated.is_some());
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_rate_limited_card_read_maps_to_a_typed_failure() {
        use axum::response::IntoResponse;
        let app = Router::new().route(
            "/task/rate",
            get(|| async {
                let mut response =
                    (axum::http::StatusCode::TOO_MANY_REQUESTS, "slow down").into_response();
                response
                    .headers_mut()
                    .insert("retry-after", "0".parse().unwrap());
                response
            }),
        );
        let client = serve(app).await;
        let failure = client.task("rate").await.unwrap_err().failure();
        assert_eq!(
            failure,
            crate::provider::ProviderFailure::RateLimited {
                retry_after_ms: None
            }
        );
    }
}
