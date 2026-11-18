//! MCP endpoint tests: the real router over an in-memory store, called as a JSON-RPC client. No Tauri window.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use relay_core::model::{NewChange, NewProject, NewProposal, StatusCategory};
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
async fn analyst_jobs_are_scoped_idempotent_and_complete() {
    let h = H::new();
    let candidate = h.task("Candidate").await;
    let other = h.task("Not a candidate").await;
    let before_tasks = h.store.list_tasks(Default::default()).unwrap().len();

    // An unknown job is a typed refusal, not a panic.
    let (err, v) = h
        .tool(ANALYST, "jobs_get", json!({"jobId": "job_missing"}))
        .await;
    assert!(err);
    assert_eq!(v["kind"], "job_not_found");

    // A real episode and job, with one candidate.
    let message = relay_core::model::RawSlackMessage {
        channel_id: "D1".into(),
        ts: "1700000000.000100".into(),
        thread_ts: None,
        user_id: Some("U1".into()),
        text: "please also run the RC build".into(),
        permalink: Some("https://slack.test/archives/D1/p1".into()),
        edited_at: None,
        deleted: false,
        historical: false,
    };
    let episode = h
        .store
        .capture_slack_message("U1", Some("Alice"), &message, 90)
        .unwrap()
        .unwrap()
        .episode_id;
    let job = h
        .store
        .create_job(relay_core::model::NewJob {
            conversation_id: episode.clone(),
            watermark: message.ts.clone(),
            kind: "classify".into(),
            project_id: Some(h.project_id.clone()),
            route: Some("change".into()),
            jev_json: Some(json!({"affects_existing": 0.9}).to_string()),
            candidates: vec![candidate.clone()],
        })
        .unwrap();
    let job_id = job.id.clone();

    let (err, v) = h.tool(ANALYST, "jobs_get", json!({"jobId": job_id})).await;
    assert!(!err, "{v}");
    assert_eq!(v["job"]["id"], job_id.as_str());
    assert_eq!(v["episode"]["episode"]["id"], episode.as_str());
    assert_eq!(v["candidates"][0]["task"]["id"], candidate.as_str());

    // A candidate write records its op; the replay returns the first result.
    let key = format!("{job_id}:0");
    let (err, v) = h
        .tool(
            ANALYST,
            "tasks_add_requirement",
            json!({
                "jobId": job_id,
                "idempotencyKey": key,
                "taskId": candidate,
                "body": "Also run the RC build",
                "sourceId": message.ts,
                "excerpt": message.text,
            }),
        )
        .await;
    assert!(!err, "{v}");
    let change_id = v["change"]["id"].as_str().unwrap().to_string();
    let (err, replay) = h
        .tool(
            ANALYST,
            "tasks_add_requirement",
            json!({
                "jobId": job_id,
                "idempotencyKey": key,
                "taskId": candidate,
                "body": "Also run the RC build",
                "sourceId": message.ts,
            }),
        )
        .await;
    assert!(!err, "{replay}");
    assert_eq!(replay["change"]["id"], change_id.as_str(), "no duplicate");
    assert_eq!(h.store.list_changes(&candidate).unwrap().len(), 1);

    // A task outside the candidate list is refused.
    let (err, v) = h
        .tool(
            ANALYST,
            "tasks_add_requirement",
            json!({
                "jobId": job_id,
                "idempotencyKey": format!("{job_id}:9"),
                "taskId": other,
                "body": "should not land",
            }),
        )
        .await;
    assert!(err);
    assert_eq!(v["kind"], "forbidden");
    assert!(h.store.list_changes(&other).unwrap().is_empty());

    // Creating a task is idempotent too.
    let create = json!({
        "jobId": job_id,
        "idempotencyKey": format!("{job_id}:2"),
        "projectId": h.project_id,
        "title": "Run the RC build",
    });
    let (err, v) = h.tool(ANALYST, "tasks_create", create.clone()).await;
    assert!(!err, "{v}");
    let created_id = v["task"]["id"].as_str().unwrap().to_string();
    let (err, replay) = h.tool(ANALYST, "tasks_create", create).await;
    assert!(!err, "{replay}");
    assert_eq!(replay["task"]["id"], created_id.as_str());
    assert_eq!(
        h.store.list_tasks(Default::default()).unwrap().len(),
        before_tasks + 1
    );

    // A proposal is limited to status/due/priority for analyst jobs.
    let (err, v) = h
        .tool(
            ANALYST,
            "proposals_submit",
            json!({
                "jobId": job_id,
                "idempotencyKey": format!("{job_id}:3"),
                "taskId": candidate,
                "kind": "change_priority",
                "baseRev": 1,
                "patch": {"priority": 1},
                "rationale": "the DM called it urgent",
            }),
        )
        .await;
    assert!(!err, "{v}");
    assert_eq!(v["proposal"]["state"], "pending");
    let (err, _) = h
        .tool(
            ANALYST,
            "proposals_submit",
            json!({
                "jobId": job_id,
                "idempotencyKey": format!("{job_id}:4"),
                "taskId": candidate,
                "kind": "change_title",
                "patch": {"title": "rewritten from a DM"},
            }),
        )
        .await;
    assert!(err, "an analyst proposal may not rewrite the title");
    assert_eq!(h.store.list_proposals(None).unwrap().len(), 1);

    // Completing closes the job.
    let (err, v) = h
        .tool(
            ANALYST,
            "jobs_complete",
            json!({"jobId": job_id, "summary": "appended one requirement"}),
        )
        .await;
    assert!(!err, "{v}");
    assert_eq!(v["state"], "done");
    assert_eq!(h.store.get_job(&job_id).unwrap().state, "done");

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

    // The read-only tools answer.
    let (err, v) = h
        .tool(ANALYST, "tasks_search", json!({"query": "Candidate"}))
        .await;
    assert!(!err, "{v}");
    assert_eq!(v["tasks"][0]["id"], candidate.as_str());
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

#[tokio::test]
async fn agents_can_reject_but_not_approve_their_proposals() {
    let h = H::new();
    let task_id = h.task("Proposal target").await;
    let task = h.store.get_task(&task_id).unwrap();
    let proposal = h
        .store
        .create_proposal(NewProposal {
            job_id: None,
            project_id: h.project_id.clone(),
            task_id: Some(task_id.clone()),
            kind: "change_priority".into(),
            base_rev: Some(task.rev),
            patch: json!({"priority": 1}),
            rationale: Some("test".into()),
            jev_json: None,
            confidence: Some(0.9),
        })
        .unwrap();
    let id = proposal.proposal.id;

    let (err, v) = h
        .tool(
            AGENT,
            "proposals_decide",
            json!({"proposalId": id, "decision": "approve"}),
        )
        .await;
    assert!(err, "an agent must not approve its own proposal");
    assert_eq!(v["kind"], "forbidden");
    assert!(
        h.store
            .list_proposals(Some("pending"))
            .unwrap()
            .iter()
            .any(|p| p.proposal.id == id),
        "the refused approve must leave the proposal pending"
    );

    let (err, v) = h
        .tool(
            AGENT,
            "proposals_decide",
            json!({"proposalId": id, "decision": "reject"}),
        )
        .await;
    assert!(!err, "reject must work: {v}");
    assert_eq!(v["proposal"]["state"], "rejected");
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
