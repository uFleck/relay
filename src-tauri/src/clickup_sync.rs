//! Tracks only the ClickUp cards assigned to the user: workspace, space and target project
//! live in `settings`, and a 10-minute schedule reconciles them. The policy is in
//! `relay_core::clickup_sync`; this adapter fetches, applies the plan and reports.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use relay_core::clickup::{ClickUpClient, ClickUpTask, Named};
use relay_core::clickup_sync::{LinkedTask, Plan, card_category, reconcile};
use relay_core::model::{
    NewLink, NewTask, ProjectStatus, StatusCategory, TaskPatch, from_unix_millis, now,
};
use relay_core::store::Store;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::secrets;

const WORKSPACE_KEY: &str = "clickup.workspaceId";
const SPACE_KEY: &str = "clickup.spaceId";
const PROJECT_KEY: &str = "clickup.projectId";
const FIRST_RUN: Duration = Duration::from_secs(30);
const INTERVAL: Duration = Duration::from_secs(10 * 60);

static RUNNING: AtomicBool = AtomicBool::new(false);
static LAST: Mutex<Option<Result<SyncResult, String>>> = Mutex::new(None);

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub created: usize,
    pub updated: usize,
    pub archived: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub workspace_id: String,
    /// `None` tracks the whole workspace. Guests have no spaces to pick from.
    pub space_id: Option<String>,
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeView {
    pub scope: Option<Scope>,
    pub last: Option<SyncResult>,
    pub error: Option<String>,
}

fn load_scope(store: &Store) -> Option<Scope> {
    let get = |key| {
        store
            .get_setting(key)
            .ok()
            .flatten()
            .filter(|v| !v.is_empty())
    };
    Some(Scope {
        workspace_id: get(WORKSPACE_KEY)?,
        space_id: get(SPACE_KEY),
        project_id: get(PROJECT_KEY)?,
    })
}

/// The last run's failure, for the Sources row.
pub fn last_error() -> Option<String> {
    LAST.lock().ok()?.as_ref()?.as_ref().err().cloned()
}

/// The first status of the project with this category, else its first `todo` status.
pub fn status_for(statuses: &[ProjectStatus], category: StatusCategory) -> Option<String> {
    let pick = |wanted| statuses.iter().find(|s| s.category == wanted);
    pick(category)
        .or_else(|| pick(StatusCategory::Todo))
        .map(|status| status.id.clone())
}

fn assigned_metadata(card: &ClickUpTask) -> String {
    serde_json::json!({ "assignedSync": true, "list": card.list_name }).to_string()
}

fn existing_links(store: &Store, project_id: &str) -> Result<Vec<LinkedTask>, String> {
    let links = store
        .list_links_in_project("clickup", project_id)
        .map_err(|error| error.to_string())?;
    Ok(links
        .into_iter()
        .filter_map(|(link, archived)| {
            let mapped = serde_json::from_str::<serde_json::Value>(&link.metadata_json)
                .ok()
                .and_then(|meta| meta.get("assignedSync")?.as_bool())
                .unwrap_or(false);
            Some(LinkedTask {
                task_id: link.task_id,
                link_id: link.id,
                external_id: link.external_id?,
                link_state: link.state,
                mapped,
                archived,
            })
        })
        .collect())
}

fn apply(store: &Store, project_id: &str, plan: Plan) -> Result<SyncResult, String> {
    let err = |error: relay_core::store::StoreError| error.to_string();
    let statuses = store.list_statuses(project_id).map_err(err)?;
    let mut result = SyncResult::default();

    for card in plan.create {
        let url = card
            .url
            .clone()
            .unwrap_or_else(|| format!("https://app.clickup.com/t/{}", card.id));
        let mut new = NewTask::new(project_id, &card.name);
        new.status_id = status_for(&statuses, card_category(&card));
        new.notes = card.description.clone();
        new.priority = card.priority.map(|priority| priority.clamp(1, 3));
        new.due_at = card.due_date.and_then(from_unix_millis);
        new.source = Some("clickup".into());
        let metadata = assigned_metadata(&card);
        let created = store.create_task(new).map_err(err)?;
        store
            .add_link(NewLink {
                task_id: created.id,
                claim_id: None,
                kind: "clickup".into(),
                external_id: Some(card.id),
                url,
                title: Some(card.name),
                state: card.status,
                metadata_json: Some(metadata),
                last_synced_at: Some(now()),
            })
            .map_err(err)?;
        result.created += 1;
    }

    for update in plan.update {
        let task = store.get_task(&update.task_id).map_err(err)?;
        let mut patch = TaskPatch::default();
        if update.unarchive {
            patch.archived = Some(false);
        }
        if let Some(category) = update.category {
            let current = statuses.iter().find(|s| s.id == task.status_id);
            if current.map(|s| s.category) != Some(category) {
                patch.status_id = status_for(&statuses, category);
            }
        }
        if task.title != update.title {
            patch.title = Some(update.title.clone());
        }
        if !patch.is_empty() {
            store
                .update_task(&update.task_id, patch, task.rev)
                .map_err(err)?;
        }
        let metadata = serde_json::json!({ "assignedSync": true }).to_string();
        store
            .set_link_sync(
                &update.link_id,
                update.state.as_deref(),
                Some(&update.title),
                Some(&metadata),
            )
            .map_err(err)?;
        result.updated += 1;
    }

    for task_id in plan.archive {
        let task = store.get_task(&task_id).map_err(err)?;
        let patch = TaskPatch {
            archived: Some(true),
            ..TaskPatch::default()
        };
        store.update_task(&task_id, patch, task.rev).map_err(err)?;
        result.archived += 1;
    }
    Ok(result)
}

async fn run_inner(store: &Store) -> Result<SyncResult, String> {
    let scope = load_scope(store).ok_or("Choose a workspace and a project first")?;
    let token = secrets::read(crate::imports::CLICKUP_TOKEN).ok_or("ClickUp is not connected")?;
    let client = ClickUpClient::new(token).map_err(|error| error.to_string())?;
    let me = client.me().await.map_err(|error| error.to_string())?;
    let assigned = client
        .assigned_tasks(&scope.workspace_id, &me, scope.space_id.as_deref())
        .await
        .map_err(|error| error.to_string())?;
    let existing = existing_links(store, &scope.project_id)?;
    let plan = reconcile(&assigned.tasks, &existing, !assigned.truncated);
    let mut result = apply(store, &scope.project_id, plan)?;
    result.truncated = assigned.truncated;
    Ok(result)
}

/// One reconcile run; overlapping runs are refused. The outcome feeds the Sources row.
pub async fn run(app: &AppHandle, store: &Store) -> Result<SyncResult, String> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("A ClickUp sync is already running".into());
    }
    let outcome = run_inner(store).await;
    RUNNING.store(false, Ordering::SeqCst);
    if let Ok(mut last) = LAST.lock() {
        *last = Some(outcome.clone());
    }
    let _ = app.emit("relay://change", ());
    if outcome.as_ref().is_ok_and(|result| result.created > 0) {
        crate::slack::reanalyze_in_background(app.clone());
    }
    outcome
}

/// Every 10 minutes while a token and a scope are set.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_RUN).await;
        loop {
            let store = app.state::<Arc<Store>>().inner().clone();
            if load_scope(&store).is_some()
                && secrets::read(crate::imports::CLICKUP_TOKEN).is_some()
            {
                let _ = run(&app, &store).await;
            }
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

fn client() -> Result<ClickUpClient, String> {
    let token = secrets::read(crate::imports::CLICKUP_TOKEN).ok_or("ClickUp is not connected")?;
    ClickUpClient::new(token).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn clickup_workspaces() -> Result<Vec<Named>, String> {
    client()?
        .workspaces()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn clickup_spaces(workspace_id: String) -> Result<Vec<Named>, String> {
    client()?
        .spaces(&workspace_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn clickup_scope(store: State<'_, Arc<Store>>) -> ScopeView {
    let last = LAST.lock().ok().and_then(|last| last.clone());
    let (last, error) = match last {
        Some(Ok(result)) => (Some(result), None),
        Some(Err(error)) => (None, Some(error)),
        None => (None, None),
    };
    ScopeView {
        scope: load_scope(&store),
        last,
        error,
    }
}

#[tauri::command]
pub fn clickup_set_scope(store: State<'_, Arc<Store>>, scope: Scope) -> Result<(), String> {
    let pairs = [
        (WORKSPACE_KEY, scope.workspace_id.trim()),
        (
            SPACE_KEY,
            scope.space_id.as_deref().unwrap_or_default().trim(),
        ),
        (PROJECT_KEY, scope.project_id.trim()),
    ];
    if pairs[0].1.is_empty() || pairs[2].1.is_empty() {
        return Err("Choose a workspace and a project".into());
    }
    store
        .get_project(pairs[2].1)
        .map_err(|error| error.to_string())?;
    for (key, value) in pairs {
        store
            .set_setting(key, value)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn clickup_sync_now(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
) -> Result<SyncResult, String> {
    run(&app, &store).await
}

#[cfg(test)]
mod tests {
    use relay_core::clickup_sync::reconcile;
    use relay_core::model::NewProject;

    use super::*;

    fn card(id: &str, status: &str) -> ClickUpTask {
        ClickUpTask {
            id: id.into(),
            name: format!("card {id}"),
            description: None,
            status: Some(status.into()),
            status_type: Some("custom".into()),
            list_name: Some("Sprint".into()),
            due_date: None,
            priority: None,
            url: None,
            date_updated: None,
            assignee_ids: Vec::new(),
        }
    }

    fn category_of(store: &Store, task_id: &str) -> StatusCategory {
        let detail = store.get_task_detail(task_id, None).unwrap();
        detail.status.category
    }

    #[test]
    fn a_sync_creates_moves_and_archives_against_a_real_store() {
        let store = Store::open_in_memory().unwrap();
        let project = store
            .create_project_with_default_workflow(NewProject::new("Inbox", "inbox", None))
            .unwrap();
        let sync = |assigned: &[ClickUpTask], complete: bool| {
            let existing = existing_links(&store, &project.project.id).unwrap();
            apply(
                &store,
                &project.project.id,
                reconcile(assigned, &existing, complete),
            )
            .unwrap()
        };

        let first = sync(
            &[
                card("1", "\u{1f468}\u{200d}\u{1f4bb} doing"),
                card("2", "todo"),
            ],
            true,
        );
        assert_eq!((first.created, first.updated, first.archived), (2, 0, 0));
        let links = store
            .list_links_in_project("clickup", &project.project.id)
            .unwrap();
        assert_eq!(links.len(), 2);
        let task_of = |external: &str| {
            links
                .iter()
                .find(|(l, _)| l.external_id.as_deref() == Some(external))
                .unwrap()
                .0
                .task_id
                .clone()
        };
        assert_eq!(category_of(&store, &task_of("1")), StatusCategory::Active);
        assert_eq!(category_of(&store, &task_of("2")), StatusCategory::Todo);

        let again = sync(
            &[
                card("1", "\u{1f468}\u{200d}\u{1f4bb} doing"),
                card("2", "todo"),
            ],
            true,
        );
        assert_eq!((again.created, again.updated, again.archived), (0, 0, 0));

        let moved = sync(&[card("1", "\u{1f91d} review"), card("2", "todo")], true);
        assert_eq!((moved.created, moved.updated, moved.archived), (0, 1, 0));
        assert_eq!(category_of(&store, &task_of("1")), StatusCategory::Review);

        let dropped = sync(&[card("2", "todo")], true);
        assert_eq!(dropped.archived, 1);
        let links = store
            .list_links_in_project("clickup", &project.project.id)
            .unwrap();
        assert!(
            links
                .iter()
                .any(|(l, archived)| l.external_id.as_deref() == Some("1") && *archived)
        );

        let back = sync(&[card("1", "\u{1f91d} review"), card("2", "todo")], true);
        assert_eq!((back.created, back.updated), (0, 1));
        let links = store
            .list_links_in_project("clickup", &project.project.id)
            .unwrap();
        assert!(links.iter().all(|(_, archived)| !archived));
    }

    /// Read-only against the real ClickUp account; the token comes from Credential Manager.
    #[tokio::test]
    #[ignore = "calls the real ClickUp API"]
    async fn real_account_assigned_cards_by_status_and_category() {
        use std::collections::BTreeMap;
        let client = client().unwrap();
        let me = client.me().await.unwrap();
        println!("me: {me}");
        for workspace in client.workspaces().await.unwrap() {
            println!("workspace {} ({})", workspace.name, workspace.id);
            let spaces = client.spaces(&workspace.id).await.unwrap();
            println!("  {} spaces: {:?}", spaces.len(), spaces);
            let mut scopes: Vec<Option<Named>> = spaces.into_iter().map(Some).collect();
            scopes.push(None);
            for space in scopes {
                let assigned = client
                    .assigned_tasks(&workspace.id, &me, space.as_ref().map(|s| s.id.as_str()))
                    .await
                    .unwrap();
                println!(
                    "  scope {:?}: {} assigned, truncated={}",
                    space.map(|s| s.name),
                    assigned.tasks.len(),
                    assigned.truncated
                );
                let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
                let mut by_category: BTreeMap<&str, usize> = BTreeMap::new();
                for card in &assigned.tasks {
                    let status = card.status.clone().unwrap_or_default();
                    *by_status.entry(status).or_default() += 1;
                    *by_category.entry(card_category(card).as_str()).or_default() += 1;
                }
                println!("    by status: {by_status:?}");
                println!("    by category: {by_category:?}");
            }
        }
    }
}
