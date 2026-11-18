//! ClickUp and GitHub Issues import (PLAN 3.6). One-shot, link-only: every imported task
//! keeps a link to its source and the next import skips it. Nothing is pushed back.

use std::path::Path;
use std::sync::Arc;

use relay_core::clickup::ClickUpClient;
use relay_core::clickup_sync::card_category;
use relay_core::github::GithubClient;
use relay_core::model::{NewLink, NewTask, StatusCategory, from_unix_millis, now};
use relay_core::repos::{self, RepoCandidate};
use relay_core::store::Store;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::secrets;

pub(crate) const CLICKUP_TOKEN: &str = "clickup-token";

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCount {
    pub imported: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorStatus {
    pub configured: bool,
}

/// The Sources row for ClickUp.
pub fn sync_source() -> relay_core::view::SyncSource {
    use relay_core::view::{SyncSource, SyncState};
    let (state, reason) = if secrets::read(CLICKUP_TOKEN).is_none() {
        (SyncState::Off, "not connected".to_string())
    } else if let Some(error) = crate::clickup_sync::last_error() {
        (SyncState::Down, error)
    } else {
        (SyncState::Ok, "token stored".to_string())
    };
    SyncSource {
        name: "ClickUp".into(),
        short: "ClickUp".into(),
        state,
        reason: Some(reason),
    }
}

#[tauri::command]
pub fn clickup_status() -> ConnectorStatus {
    ConnectorStatus {
        configured: secrets::read(CLICKUP_TOKEN).is_some(),
    }
}

#[tauri::command]
pub async fn clickup_set_token(token: String) -> Result<ConnectorStatus, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("the ClickUp token is empty".into());
    }
    let client = ClickUpClient::new(token.clone()).map_err(|error| error.to_string())?;
    client
        .check()
        .await
        .map_err(|error| format!("ClickUp rejected the token: {error}"))?;
    secrets::write(CLICKUP_TOKEN, &token)?;
    Ok(ConnectorStatus { configured: true })
}

#[tauri::command]
pub fn clickup_clear_token() -> ConnectorStatus {
    secrets::delete(CLICKUP_TOKEN);
    ConnectorStatus::default()
}

#[tauri::command]
pub async fn clickup_import(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    project_id: String,
    list_id: String,
    include_closed: Option<bool>,
) -> Result<ImportCount, String> {
    let token = secrets::read(CLICKUP_TOKEN).ok_or("ClickUp is not connected")?;
    let client = ClickUpClient::new(token).map_err(|error| error.to_string())?;
    let tasks = client
        .list_tasks(list_id.trim(), include_closed.unwrap_or(false))
        .await
        .map_err(|error| error.to_string())?;
    let statuses = store
        .list_statuses(&project_id)
        .map_err(|error| error.to_string())?;

    let mut count = ImportCount::default();
    for task in tasks {
        let url = task
            .url
            .clone()
            .unwrap_or_else(|| format!("https://app.clickup.com/t/{}", task.id));
        if store
            .find_link("clickup", Some(&task.id), &url)
            .map_err(|error| error.to_string())?
            .is_some()
        {
            count.skipped += 1;
            continue;
        }
        let status_id = crate::clickup_sync::status_for(&statuses, card_category(&task));
        let mut new = NewTask::new(&project_id, &task.name);
        new.notes = task.description;
        new.status_id = status_id;
        new.priority = task.priority.map(|priority| priority.clamp(1, 3));
        new.due_at = task.due_date.and_then(from_unix_millis);
        new.source = Some("clickup".into());
        let created = store.create_task(new).map_err(|error| error.to_string())?;
        store
            .add_link(NewLink {
                task_id: created.id.clone(),
                claim_id: None,
                kind: "clickup".into(),
                external_id: Some(task.id),
                url,
                title: Some(created.title),
                state: task.status,
                metadata_json: None,
                last_synced_at: Some(now()),
            })
            .map_err(|error| error.to_string())?;
        count.imported += 1;
    }
    let _ = app.emit("relay://change", ());
    Ok(count)
}

#[tauri::command]
pub async fn github_import_issues(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    project_id: String,
    owner: String,
    repo: String,
) -> Result<ImportCount, String> {
    let client = GithubClient::new(crate::github::current_token().await)
        .map_err(|error| error.to_string())?;
    let issues = client
        .list_issues(owner.trim(), repo.trim())
        .await
        .map_err(|error| error.to_string())?;
    let statuses = store
        .list_statuses(&project_id)
        .map_err(|error| error.to_string())?;
    let todo = statuses
        .iter()
        .find(|status| status.category == StatusCategory::Todo)
        .map(|status| status.id.clone());

    let mut count = ImportCount::default();
    for issue in issues {
        let external_id = format!("{}/{}#{}", owner.trim(), repo.trim(), issue.number);
        if store
            .find_link("github", Some(&external_id), &issue.url)
            .map_err(|error| error.to_string())?
            .is_some()
        {
            count.skipped += 1;
            continue;
        }
        let mut new = NewTask::new(&project_id, &issue.title);
        new.notes = issue.body;
        new.status_id = todo.clone();
        new.source = Some("github".into());
        let created = store.create_task(new).map_err(|error| error.to_string())?;
        store
            .add_link(NewLink {
                task_id: created.id.clone(),
                claim_id: None,
                kind: "github".into(),
                external_id: Some(external_id),
                url: issue.url,
                title: Some(created.title),
                state: Some(issue.state),
                metadata_json: Some(serde_json::json!({ "labels": issue.labels }).to_string()),
                last_synced_at: Some(now()),
            })
            .map_err(|error| error.to_string())?;
        count.imported += 1;
    }
    let _ = app.emit("relay://change", ());
    Ok(count)
}

#[tauri::command]
pub async fn repos_scan(
    store: State<'_, Arc<Store>>,
    folder: String,
) -> Result<Vec<RepoCandidate>, String> {
    let store = store.inner().clone();
    // One `git` call per subfolder; not on the async threads.
    tauri::async_runtime::spawn_blocking(move || {
        let found = repos::scan_folder(Path::new(folder.trim())).map_err(|e| e.to_string())?;
        repos::annotate_projects(&store, found).map_err(|e| e.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn repos_import(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    folder: String,
) -> Result<ImportCount, String> {
    let store = store.inner().clone();
    let count = tauri::async_runtime::spawn_blocking(move || {
        let found = repos::scan_folder(Path::new(folder.trim())).map_err(|e| e.to_string())?;
        repos::import_repos(&store, &found).map_err(|e| e.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;
    let _ = app.emit("relay://change", ());
    if count.imported > 0 {
        crate::slack::reanalyze_in_background(app.clone());
    }
    crate::github::sync_in_background(app);
    Ok(ImportCount {
        imported: count.imported,
        skipped: count.skipped,
    })
}
