//! Thin Grilling commands and the per-turn worker (#35). All decisions live in
//! `relay_core::v2::grilling`; this module only owns processes, the loopback adapter and events.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use relay_core::v2::{
    self, GrillEntry, GrillModelResult, GrillStatus, V2Error, V2Store, answer, approve_brief,
    assistant_text, begin_grill_attempt, build_reference_plan, cancel_grill, complete_brief,
    dispatch_readiness, fail_grill_turn, grill_continue_link_only, grill_go_enabled,
    parse_grill_result, record_question, recover_interrupted_grills, retry_grill,
    sanitize_stage_error, terminal_outcome,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::CommandError;
use crate::grilling_mcp::RunningGrill;
use crate::opencode::{AttemptInput, BoundaryCache, GrillServerConfig, run_isolated};

/// One exchange in the Task's retained transcript.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrillExchangeView {
    pub revision: i64,
    pub entries: Vec<GrillEntry>,
}

/// The single read behind both inline Grilling panels. No token, bearer or Reference content
/// reaches the frontend: only identities, the saved exchange and the gates.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrillView {
    pub task_id: String,
    pub mode: String,
    pub task_status: String,
    pub revision: Option<i64>,
    pub status: Option<String>,
    pub question: Option<String>,
    pub entries: Vec<GrillEntry>,
    pub exchanges: Vec<GrillExchangeView>,
    pub missing_references: Vec<String>,
    pub can_answer: bool,
    pub can_retry: bool,
    pub can_cancel: bool,
    pub can_go: bool,
    pub dispatch_ready: bool,
    pub project_id: Option<String>,
    pub rendered: Option<String>,
    pub pending_action: Option<String>,
    pub failure_stage: Option<String>,
    pub failure_reason: Option<String>,
    pub updated_at: Option<String>,
}

fn build_view(store: &Arc<V2Store>, task_id: &str) -> Result<GrillView, V2Error> {
    let state = v2::task_line_state(store, task_id)?;
    let record = store.grill(task_id)?;
    let brief = store.brief(task_id)?;
    let plan = build_reference_plan(
        &state.references,
        brief
            .as_ref()
            .map(|b| b.link_only_refs.as_slice())
            .unwrap_or(&[]),
    );
    let missing_references = plan.missing.iter().map(|r| r.identity.clone()).collect();
    let exchange = record.as_ref();
    let status = exchange.map(|record| record.status);
    let signature_clean = exchange.is_some_and(|record| {
        record.source_signature == v2::source_signature(&state.line_text, &state.references)
    });
    let all = store.grills(task_id)?;
    Ok(GrillView {
        task_id: task_id.to_string(),
        mode: state.task.agent_mode.as_str().to_string(),
        task_status: state.task.status.as_str().to_string(),
        revision: exchange.map(|record| record.revision),
        status: status.map(|value| value.as_str().to_string()),
        question: exchange.and_then(|record| record.question.clone()),
        entries: exchange
            .map(|record| record.entries.clone())
            .unwrap_or_default(),
        exchanges: all
            .into_iter()
            .map(|record| GrillExchangeView {
                revision: record.revision,
                entries: record.entries,
            })
            .collect(),
        missing_references,
        can_answer: status == Some(GrillStatus::Question) && signature_clean,
        can_retry: status == Some(GrillStatus::Failed),
        can_cancel: !matches!(status, Some(GrillStatus::Approved | GrillStatus::Cancelled)),
        can_go: grill_go_enabled(store, task_id)?,
        dispatch_ready: dispatch_readiness(store, task_id)?.is_some(),
        project_id: state.task.project_id.clone(),
        rendered: brief.and_then(|record| record.rendered),
        pending_action: state.task.pending_action.clone(),
        failure_stage: exchange
            .and_then(|record| record.failure_stage.clone())
            .or_else(|| state.task.failure_stage.clone()),
        failure_reason: exchange
            .and_then(|record| record.failure_reason.clone())
            .or_else(|| state.task.failure_reason.clone()),
        updated_at: exchange.map(|record| record.updated_at.clone()),
    })
}

fn publish(app: &AppHandle, task_id: &str) {
    let _ = app.emit("relay://grilling", task_id.to_string());
    let _ = app.emit("relay://v2-change", task_id.to_string());
}

/// Coalesces competing starts from the Dock and the Task sheet and owns live listener handles.
#[derive(Default)]
pub struct GrillingState {
    inflight: Mutex<HashSet<String>>,
    servers: Mutex<HashMap<String, RunningGrill>>,
    boundary: BoundaryCache,
}

struct AttemptGuard<'a> {
    task_id: String,
    inflight: &'a Mutex<HashSet<String>>,
}

impl Drop for AttemptGuard<'_> {
    fn drop(&mut self) {
        self.inflight
            .lock()
            .expect("grilling inflight mutex poisoned")
            .remove(&self.task_id);
    }
}

impl GrillingState {
    fn begin(&self, task_id: &str) -> Option<AttemptGuard<'_>> {
        if !self
            .inflight
            .lock()
            .expect("grilling inflight mutex poisoned")
            .insert(task_id.to_string())
        {
            return None;
        }
        Some(AttemptGuard {
            task_id: task_id.to_string(),
            inflight: &self.inflight,
        })
    }

    fn retire(&self, task_id: &str) {
        if let Some(mut server) = self
            .servers
            .lock()
            .expect("grilling servers mutex poisoned")
            .remove(task_id)
        {
            server.stop();
        }
    }
}

/// Starts one Grilling exchange after the core lifecycle selects that stage.
pub fn start_task(app: &AppHandle, task_id: &str) {
    let state = app.state::<GrillingState>();
    if state.begin(task_id).is_some() {
        spawn_work(
            app.clone(),
            task_id.to_string(),
            Work::Start,
            state.boundary.clone(),
        );
    }
}

/// Marks attempts abandoned by a restart as retryable and announces the change.
pub fn init(app: &AppHandle) {
    let store = app.state::<Arc<V2Store>>();
    match recover_interrupted_grills(store.inner()) {
        Ok(0) => {}
        Ok(count) => {
            eprintln!("relay: recovered {count} interrupted Grilling attempt(s)");
            let _ = app.emit("relay://grilling", String::new());
            let _ = app.emit("relay://v2-change", String::new());
        }
        Err(error) => eprintln!("relay: could not recover Grilling attempts: {error}"),
    }
}

#[tauri::command(async)]
pub fn v2_grilling_view(
    task_id: String,
    store: State<'_, Arc<V2Store>>,
) -> Result<GrillView, CommandError> {
    Ok(build_view(store.inner(), &task_id)?)
}

#[tauri::command(async)]
pub fn v2_grilling_start(
    task_id: String,
    app: AppHandle,
    state: State<'_, GrillingState>,
    store: State<'_, Arc<V2Store>>,
) -> Result<GrillView, CommandError> {
    if state.begin(&task_id).is_some() {
        spawn_work(app, task_id.clone(), Work::Start, state.boundary.clone());
    }
    Ok(build_view(store.inner(), &task_id)?)
}

#[tauri::command(async)]
pub fn v2_grilling_answer(
    task_id: String,
    revision: i64,
    answer_text: String,
    app: AppHandle,
    state: State<'_, GrillingState>,
    store: State<'_, Arc<V2Store>>,
) -> Result<GrillView, CommandError> {
    let turn = answer(store.inner(), &task_id, revision, &answer_text)?;
    if state.begin(&task_id).is_some() {
        spawn_work(
            app,
            task_id.clone(),
            Work::Turn(turn),
            state.boundary.clone(),
        );
    }
    Ok(build_view(store.inner(), &task_id)?)
}

#[tauri::command(async)]
pub fn v2_grilling_retry(
    task_id: String,
    app: AppHandle,
    state: State<'_, GrillingState>,
    store: State<'_, Arc<V2Store>>,
) -> Result<GrillView, CommandError> {
    let Some(turn) = retry_grill(store.inner(), &task_id)? else {
        return Ok(build_view(store.inner(), &task_id)?);
    };
    if state.begin(&task_id).is_some() {
        spawn_work(
            app,
            task_id.clone(),
            Work::Turn(turn),
            state.boundary.clone(),
        );
    }
    Ok(build_view(store.inner(), &task_id)?)
}

#[tauri::command(async)]
pub fn v2_grilling_cancel(
    task_id: String,
    app: AppHandle,
    state: State<'_, GrillingState>,
    store: State<'_, Arc<V2Store>>,
) -> Result<GrillView, CommandError> {
    cancel_grill(store.inner(), &task_id)?;
    state.retire(&task_id);
    publish(&app, &task_id);
    Ok(build_view(store.inner(), &task_id)?)
}

#[tauri::command(async)]
pub fn v2_grilling_go(
    task_id: String,
    revision: i64,
    edited: Option<String>,
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<GrillView, CommandError> {
    approve_brief(store.inner(), &task_id, revision, edited)?;
    crate::lifecycle::advance(&app, &task_id);
    publish(&app, &task_id);
    Ok(build_view(store.inner(), &task_id)?)
}

#[tauri::command(async)]
pub fn v2_grilling_continue_link_only(
    task_id: String,
    revision: i64,
    identity: String,
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<GrillView, CommandError> {
    grill_continue_link_only(store.inner(), &task_id, revision, &identity)?;
    publish(&app, &task_id);
    Ok(build_view(store.inner(), &task_id)?)
}

enum Work {
    Start,
    Turn(v2::GrillTurnInput),
}

fn spawn_work(app: AppHandle, task_id: String, work: Work, boundary: BoundaryCache) {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Arc<V2Store>>().inner().clone();
        let input = match work {
            Work::Start => match begin_grill_attempt(&store, &task_id) {
                Ok(Some(input)) => input,
                Ok(None) => {
                    publish(&app, &task_id);
                    return;
                }
                Err(error) => {
                    let _ = fail_grill_turn(&store, &task_id, "", &error.to_string());
                    publish(&app, &task_id);
                    return;
                }
            },
            Work::Turn(input) => input,
        };
        run_turn(&app, &store, &input, &boundary);
        publish(&app, &task_id);
    });
}

fn run_turn(
    app: &AppHandle,
    store: &Arc<V2Store>,
    input: &v2::GrillTurnInput,
    boundary: &BoundaryCache,
) {
    let endpoint = match RunningGrill::start(
        store.clone(),
        input.task_id.clone(),
        input.attempt_id.clone(),
    ) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            let _ = fail_grill_turn(store, &input.task_id, &input.attempt_id, &error);
            return;
        }
    };
    if let Some(state) = app.try_state::<GrillingState>() {
        state
            .servers
            .lock()
            .expect("grilling servers mutex poisoned")
            .insert(input.task_id.clone(), endpoint);
    }
    let result = run_with_endpoint(app, store, input, boundary);
    if let Err(reason) = result {
        let _ = fail_grill_turn(store, &input.task_id, &input.attempt_id, &reason);
    }
    if let Some(state) = app.try_state::<GrillingState>() {
        state.retire(&input.task_id);
    }
}

fn run_with_endpoint(
    app: &AppHandle,
    store: &Arc<V2Store>,
    input: &v2::GrillTurnInput,
    boundary: &BoundaryCache,
) -> Result<(), String> {
    let server = app
        .try_state::<GrillingState>()
        .and_then(|state| {
            state
                .servers
                .lock()
                .expect("grilling servers mutex poisoned")
                .get(&input.task_id)
                .map(|server| (server.url(), server.bearer()))
        })
        .ok_or_else(|| "the Grilling adapter was cancelled".to_string())?;
    let grill = GrillServerConfig {
        url: server.0,
        bearer: server.1,
    };
    let stdout = run_isolated(
        app,
        store,
        AttemptInput {
            task_id: input.task_id.clone(),
            attempt_id: input.attempt_id.clone(),
            prompt: input.prompt.clone(),
            agent: "relay-grill",
            scratch_group: "grilling",
            grill: Some(grill),
        },
        boundary,
    )?;
    terminal_outcome(&stdout).map_err(|reason| sanitize_stage_error(&reason))?;
    let text = assistant_text(&stdout);
    match parse_grill_result(&text) {
        Ok(GrillModelResult::Question(_)) => {
            record_question(store, &input.task_id, &input.attempt_id, &text)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        Ok(GrillModelResult::Brief(_)) => {
            complete_brief(store, &input.task_id, &input.attempt_id, &text)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_fields_are_serialized_with_the_ipc_names() {
        let value = serde_json::to_value(GrillView {
            task_id: "t1".into(),
            mode: "grill".into(),
            task_status: "grilling".into(),
            revision: Some(1),
            status: Some("question".into()),
            question: Some("Which format?".into()),
            entries: vec![GrillEntry {
                role: v2::GrillRole::Question,
                text: "Which format?".into(),
                created_at: "2026-11-21T00:00:00.000Z".into(),
            }],
            exchanges: vec![],
            missing_references: vec![],
            can_answer: true,
            can_retry: false,
            can_cancel: true,
            can_go: false,
            dispatch_ready: false,
            project_id: None,
            rendered: None,
            pending_action: None,
            failure_stage: None,
            failure_reason: None,
            updated_at: None,
        })
        .unwrap();
        assert_eq!(value["taskId"], "t1");
        assert_eq!(value["canAnswer"], true);
        assert_eq!(value["entries"][0]["role"], "question");
        assert!(value.get("token").is_none());
    }
}
