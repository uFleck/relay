//! MCP endpoint tests: the real router over an in-memory store, called as a JSON-RPC client. No Tauri window.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use relay_core::model::{NewChange, NewProject, StatusCategory};
use relay_core::store::Store;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    analyst, load_tokens,
    server::{self, MAX_BODY, Tokens},
    tools,
};

const PORT: u16 = 38547;
const AGENT: &str = "/mcp";
const ANALYST: &str = "/mcp/analyst";

struct H {
    app: Router,
    store: Arc<Store>,
    tokens: Tokens,
    project_id: String,
}

impl H {
    fn new() -> Self {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let project = store
            .create_project_with_default_workflow(NewProject::new("Inbox", "inbox", None))
            .unwrap();
        let tokens = Tokens {
            agent: "a".repeat(32),
            analyst: "b".repeat(32),
        };
        Self {
            app: server::router(store.clone(), tokens.clone(), PORT),
            store,
            tokens,
            project_id: project.project.id,
        }
    }

    fn token(&self, path: &str) -> &str {
        if path == ANALYST {
            &self.tokens.analyst
        } else {
            &self.tokens.agent
        }
    }

    async fn send(&self, path: &str, token: Option<&str>, body: Vec<u8>) -> (StatusCode, Vec<u8>) {
        let mut req = Request::post(path)
            .header(header::HOST, format!("127.0.0.1:{PORT}"))
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let res = self
            .app
            .clone()
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, bytes.to_vec())
    }

    async fn rpc(&self, path: &str, method: &str, params: Value) -> Value {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let (status, bytes) = self
            .send(path, Some(self.token(path)), body.to_string().into_bytes())
            .await;
        assert_eq!(status, StatusCode::OK);
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Calls a tool; returns `(isError, parsed JSON text)`.
    async fn tool(&self, path: &str, name: &str, args: Value) -> (bool, Value) {
        let v = self
            .rpc(path, "tools/call", json!({"name": name, "arguments": args}))
            .await;
        let result = &v["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: no tool result in {v}"));
        (
            result["isError"].as_bool().unwrap(),
            serde_json::from_str(text).unwrap(),
        )
    }

    async fn ok(&self, name: &str, args: Value) -> Value {
        let (err, v) = self.tool(AGENT, name, args).await;
        assert!(!err, "{name} failed: {v}");
        v
    }

    async fn task(&self, title: &str) -> String {
        let task = self
            .ok(
                "tasks_create",
                json!({"projectId": self.project_id, "title": title}),
            )
            .await;
        task["id"].as_str().unwrap().to_owned()
    }

    fn status_of(&self, category: StatusCategory) -> String {
        self.store
            .list_statuses(&self.project_id)
            .unwrap()
            .into_iter()
            .find(|s| s.category == category)
            .unwrap()
            .id
    }
}

#[tokio::test]
async fn initialize_names_each_endpoint_and_explains_the_flow() {
    let h = H::new();
    let v = h
        .rpc(
            AGENT,
            "initialize",
            json!({"protocolVersion": "2025-06-18"}),
        )
        .await;
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(v["result"]["serverInfo"]["name"], "relay");
    let instructions = v["result"]["instructions"].as_str().unwrap();
    for word in [
        "tasks_claim",
        "tasks_progress",
        "tasks_complete",
        "untrusted",
    ] {
        assert!(instructions.contains(word), "{word}");
    }

    let v = h.rpc(ANALYST, "initialize", json!({})).await;
    assert_eq!(v["result"]["serverInfo"]["name"], "relay-analyst");
    assert_eq!(h.rpc(AGENT, "ping", json!({})).await["result"], json!({}));
}

#[tokio::test]
async fn tools_list_is_per_endpoint() {
    let h = H::new();
    let names = |v: Value| -> Vec<String> {
        v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let agent = names(h.rpc(AGENT, "tools/list", json!({})).await);
    assert!(agent.contains(&"tasks_claim".to_owned()));
    assert_eq!(agent, tools::NAMES);

    let analyst = names(h.rpc(ANALYST, "tools/list", json!({})).await);
    assert_eq!(analyst, analyst::ALLOWED);
    assert!(!analyst.contains(&"tasks_claim".to_owned()));
}

#[tokio::test]
async fn missing_or_wrong_token_is_401_with_no_body() {
    let h = H::new();
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})
        .to_string()
        .into_bytes();
    for (path, token) in [
        (AGENT, None),
        (AGENT, Some("wrong")),
        (ANALYST, None),
        (ANALYST, Some("wrong")),
        // Each token opens only its own endpoint.
        (ANALYST, Some(h.tokens.agent.as_str())),
        (AGENT, Some(h.tokens.analyst.as_str())),
    ] {
        let (status, bytes) = h.send(path, token, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path} {token:?}");
        assert!(bytes.is_empty(), "{path}: 401 must carry no detail");
    }
}

#[tokio::test]
async fn foreign_host_is_refused() {
    let h = H::new();
    let req = Request::post(AGENT)
        .header(header::HOST, format!("evil.test:{PORT}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", h.tokens.agent))
        .body(Body::from("{}"))
        .unwrap();
    let res = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn analyst_token_cannot_call_agent_only_tools() {
    let h = H::new();
    let task_id = h.task("Ship the hover").await;
    for name in tools::NAMES
        .iter()
        .filter(|n| !analyst::ALLOWED.contains(n))
    {
        let v = h
            .rpc(
                ANALYST,
                "tools/call",
                json!({"name": name, "arguments": {"taskId": task_id, "claimId": "x"}}),
            )
            .await;
        assert_eq!(v["error"]["code"], -32602, "{name}: {v}");
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not allowed"),
            "{name}: {v}"
        );
    }
    // Nothing the refused calls named was touched.
    assert!(
        h.store
            .get_task_detail(&task_id, None)
            .unwrap()
            .active_claim
            .is_none()
    );
}

#[tokio::test]
async fn analyst_job_scoped_calls_are_refused_without_a_job_store() {
    let h = H::new();
    let task_id = h.task("Candidate").await;
    let before = h.store.list_tasks(Default::default()).unwrap().len();
    let job = "job_1";
    for (name, args) in [
        ("jobs_get", json!({"jobId": job})),
        ("jobs_complete", json!({"jobId": job})),
        ("tasks_get", json!({"jobId": job, "id": task_id})),
        (
            "tasks_create",
            json!({"jobId": job, "idempotencyKey": "job_1:0", "projectId": h.project_id, "title": "From Slack"}),
        ),
        (
            "tasks_add_requirement",
            json!({"jobId": job, "idempotencyKey": "job_1:1", "taskId": task_id, "body": "Works on RC"}),
        ),
        (
            "proposals_submit",
            json!({"jobId": job, "taskId": task_id, "kind": "change_priority", "patch": {"priority": 1}}),
        ),
    ] {
        let (err, v) = h.tool(ANALYST, name, args).await;
        assert!(err, "{name} must be refused: {v}");
        assert_eq!(v["kind"], "job_store_unavailable", "{name}");
    }
    // A write without a jobId does not even parse.
    let (err, v) = h
        .tool(
            ANALYST,
            "tasks_create",
            json!({"projectId": h.project_id, "title": "x"}),
        )
        .await;
    assert!(err);
    assert_eq!(v["kind"], "invalid_args");

    assert_eq!(
        h.store.list_tasks(Default::default()).unwrap().len(),
        before
    );
    assert!(h.store.list_changes(&task_id).unwrap().is_empty());
    assert!(h.store.list_proposals(None).unwrap().is_empty());

    // The read-only tools answer.
    let (err, v) = h
        .tool(ANALYST, "tasks_search", json!({"query": "Candidate"}))
        .await;
    assert!(!err, "{v}");
    assert_eq!(v["tasks"][0]["id"], task_id.as_str());
    let (err, _) = h.tool(ANALYST, "projects_list", json!({})).await;
    assert!(!err);
}

#[tokio::test]
async fn claim_progress_ack_complete_lands_in_review() {
    let h = H::new();
    let task_id = h.task("Fix the updater").await;
    let claim = h
        .ok(
            "tasks_claim",
            json!({"taskId": task_id, "holderName": "T3 Code", "threadRef": "thr_1"}),
        )
        .await;
    let claim_id = claim["claim"]["id"].as_str().unwrap().to_owned();
    assert_eq!(claim["unackedChanges"], json!([]));

    // A requirement arrives from Slack while the agent works.
    let mut change =
        NewChange::requirement(&task_id, "Also needs to work for the RC build", "slack");
    change.excerpt = Some("Also needs to work for the RC build.".into());
    let change = h.store.add_change(change).unwrap();

    let progress = h
        .ok(
            "tasks_progress",
            json!({"claimId": claim_id, "note": "Updater wired"}),
        )
        .await;
    let unacked = progress["unackedChanges"].as_array().unwrap();
    assert_eq!(unacked.len(), 1);
    assert_eq!(unacked[0]["change"]["id"], change.id.as_str());

    let detail = h
        .ok("tasks_get", json!({"id": task_id, "claimId": claim_id}))
        .await;
    assert_eq!(detail["unackedChanges"].as_array().unwrap().len(), 1);

    let ack = h
        .ok(
            "tasks_ack_change",
            json!({"claimId": claim_id, "changeId": change.id}),
        )
        .await;
    assert_eq!(ack["unackedChanges"], json!([]));
    let changes = h.ok("tasks_changes", json!({"claimId": claim_id})).await;
    assert_eq!(changes["unackedChanges"], json!([]));

    let link = h
        .ok(
            "tasks_link",
            json!({"claimId": claim_id, "kind": "pr", "url": "https://github.com/uFleck/relay/pull/1", "externalId": "1"}),
        )
        .await;
    assert_eq!(link["link"]["taskId"], task_id.as_str());

    let task = h
        .ok(
            "tasks_complete",
            json!({"claimId": claim_id, "summary": "Updater checks on start"}),
        )
        .await;
    assert_eq!(
        task["statusId"],
        h.status_of(StatusCategory::Review).as_str()
    );

    // The claim is gone: a later claimant write fails.
    let (err, v) = h
        .tool(
            AGENT,
            "tasks_progress",
            json!({"claimId": claim_id, "note": "late"}),
        )
        .await;
    assert!(err);
    assert_eq!(v["kind"], "claim_released");

    let activity = h.ok("activity_list", json!({"limit": 3})).await;
    assert_eq!(activity["events"].as_array().unwrap().len(), 3);
    assert_eq!(activity["events"][0]["taskTitle"], "Fix the updater");
}

#[tokio::test]
async fn second_claim_gets_a_conflict_with_the_holder() {
    let h = H::new();
    let task_id = h.task("Contended").await;
    let first = h
        .ok(
            "tasks_claim",
            json!({"taskId": task_id, "holderName": "one"}),
        )
        .await;
    let (err, v) = h
        .tool(
            AGENT,
            "tasks_claim",
            json!({"taskId": task_id, "holderName": "two"}),
        )
        .await;
    assert!(err);
    assert_eq!(v["kind"], "claim_conflict");
    assert_eq!(v["holder"]["id"], first["claim"]["id"]);
    assert_eq!(v["holder"]["holderName"], "one");
}

#[tokio::test]
async fn update_cannot_mark_done_and_get_checks_claim_ownership() {
    let h = H::new();
    let task_id = h.task("Guarded").await;
    let other = h.task("Other").await;
    let (err, v) = h
        .tool(
            AGENT,
            "tasks_update",
            json!({"id": task_id, "expectedRev": 1, "statusId": h.status_of(StatusCategory::Done)}),
        )
        .await;
    assert!(err);
    assert_eq!(v["kind"], "forbidden");

    let (err, v) = h
        .tool(
            AGENT,
            "tasks_update",
            json!({"id": task_id, "expectedRev": 1, "completedAt": "2026-11-17T00:00:00Z"}),
        )
        .await;
    assert!(err);
    assert_eq!(v["kind"], "invalid_args");

    let updated = h
        .ok(
            "tasks_update",
            json!({"id": task_id, "expectedRev": 1, "title": "Guarded!", "priority": 2}),
        )
        .await;
    assert_eq!(updated["rev"], 2);
    let (err, v) = h
        .tool(
            AGENT,
            "tasks_update",
            json!({"id": task_id, "expectedRev": 1, "title": "stale"}),
        )
        .await;
    assert!(err);
    assert_eq!(v["kind"], "stale_rev");

    let claim = h.ok("tasks_claim", json!({"taskId": other})).await;
    let (err, v) = h
        .tool(
            AGENT,
            "tasks_get",
            json!({"id": task_id, "claimId": claim["claim"]["id"]}),
        )
        .await;
    assert!(err);
    assert_eq!(v["kind"], "claim_task_mismatch");
}

#[tokio::test]
async fn unknown_tool_and_unknown_method_are_rpc_errors() {
    let h = H::new();
    let v = h
        .rpc(
            AGENT,
            "tools/call",
            json!({"name": "tasks_delete", "arguments": {}}),
        )
        .await;
    assert_eq!(v["error"]["code"], -32602);
    let v = h
        .rpc(
            ANALYST,
            "tools/call",
            json!({"name": "tasks_delete", "arguments": {}}),
        )
        .await;
    assert_eq!(v["error"]["code"], -32602);
    let v = h.rpc(AGENT, "resources/list", json!({})).await;
    assert_eq!(v["error"]["code"], -32601);
}

#[tokio::test]
async fn sync_is_honest_about_no_connectors() {
    let h = H::new();
    assert_eq!(
        h.ok("sync_status", json!({})).await["connectors"],
        json!([])
    );
    let (err, v) = h.tool(AGENT, "sync_now", json!({})).await;
    assert!(err);
    assert_eq!(v["kind"], "not_configured");
}

#[tokio::test]
async fn body_over_1mb_is_rejected() {
    let h = H::new();
    let (status, _) = h
        .send(AGENT, Some(&h.tokens.agent), vec![b' '; MAX_BODY + 1])
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn taken_port_fails_loudly() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let tokens = Tokens {
        agent: "a".repeat(32),
        analyst: "b".repeat(32),
    };
    let err = server::serve(store, tokens, port)
        .await
        .err()
        .expect("a held port must not be served");
    assert!(err.contains(&port.to_string()), "{err}");
}

#[tokio::test]
async fn serves_over_a_real_socket() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let tokens = Tokens {
        agent: "a".repeat(32),
        analyst: "b".repeat(32),
    };
    let _running = server::serve(store, tokens.clone(), port).await.unwrap();

    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        tokens.agent,
        body.len()
    );
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"tasks_claim\""));
}

#[test]
fn tokens_are_created_once_and_distinct() {
    let dir = std::env::temp_dir().join(format!("relay-mcp-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let first = load_tokens(&dir).unwrap();
    assert_eq!(first.agent.len(), 32);
    assert_ne!(first.agent, first.analyst);
    let again = load_tokens(&dir).unwrap();
    assert_eq!(again.agent, first.agent);
    assert_eq!(again.analyst, first.analyst);
    let _ = std::fs::remove_dir_all(&dir);
}
