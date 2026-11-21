//! v2 Settings boundary. Policy lives in `relay-core`; this module only reads/writes the
//! v2 store and keyring, probes providers and shapes secret-free status DTOs.
//!
//! Credentials never come back from a command: a `ProviderStatus` carries a source, a
//! status, a fixed or sanitized reason and an optional last-four mask. v1 discovery,
//! PR polling and Slack reanalysis are deliberately not called from here.

use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use relay_core::clickup::ClickUpClient;
use relay_core::github::GithubClient;
use relay_core::repos::{self, RepoCandidate};
use relay_core::v2::{
    NewV2Project, T3Target, V2ImportScope, V2Project, V2Settings, V2SettingsUpdate, V2Store,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::CommandError;
use crate::github;
use crate::secrets;
use crate::{slack, t3};

const GITHUB_KEY: &str = "github-token";
const CLICKUP_KEY: &str = "clickup-token";
const OPENCODE_KEY: &str = "opencode-key";
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const SETTINGS_CHANGE_EVENT: &str = "relay://settings-change";
pub const PROVIDER_STATUS_EVENT: &str = "relay://settings-status";

/// The four actionable states the Settings sections render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderState {
    MissingSetup,
    ConfiguredUnverified,
    Verified,
    Unavailable,
}

/// Where the connection comes from. `Relay` is a v2 Credential Manager entry;
/// `ExternalSession` names a detected `gh` login or Claude sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSource {
    Relay,
    ExternalSession,
    /// Reserved for the Slack connector once #34/#31 can read its results.
    #[allow(dead_code)]
    Connector,
    Cli,
    None,
}

/// The whole credential-facing contract. No raw value is present.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub source: ProviderSource,
    pub status: ProviderState,
    pub reason: Option<String>,
    pub last_four: Option<String>,
}

pub type ProviderStatusMap = BTreeMap<String, ProviderStatus>;

/// A written mapping check result. Reasons are fixed strings from `t3.rs`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct T3MappingStatus {
    pub status: ProviderState,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct T3TargetInput {
    pub kind: String,
    pub value: String,
}

/// Backend-only OpenCode configuration for #34. Deliberately not Serialize and not a
/// Tauri command: the key is raw so a Rust adapter can use it, and must never be logged.
#[allow(dead_code)]
pub struct OpencodeConfig {
    pub model: Option<String>,
    pub key: Option<String>,
}

/// The last four characters of a Relay-stored credential, masked for display.
fn last_four(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let tail: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Some(format!("••••{tail}"))
}

fn unavailable(reason: &'static str) -> ProviderStatus {
    ProviderStatus {
        source: ProviderSource::None,
        status: ProviderState::Unavailable,
        reason: Some(reason.into()),
        last_four: None,
    }
}

/// Cached so a status read is instant and a slow provider cannot block the UI.
#[derive(Default)]
pub struct ProviderCache {
    statuses: Mutex<ProviderStatusMap>,
}

impl ProviderCache {
    pub fn snapshot(&self) -> ProviderStatusMap {
        self.statuses.lock().expect("provider status").clone()
    }

    fn put(&self, key: &str, status: ProviderStatus) {
        self.statuses
            .lock()
            .expect("provider status")
            .insert(key.to_string(), status);
    }

    fn replace(&self, statuses: ProviderStatusMap) {
        *self.statuses.lock().expect("provider status") = statuses;
    }
}

/// Installs the cache and fills it without any network call. Live probes happen on
/// explicit refresh or after a credential write, never during startup.
pub fn start(app: &AppHandle) {
    app.manage(ProviderCache::default());
    let store = app.state::<Arc<V2Store>>().inner().clone();
    let mut statuses = ProviderStatusMap::new();
    statuses.insert("github".into(), offline_github(&store));
    statuses.insert("clickup".into(), offline_clickup(&store));
    statuses.insert("slack".into(), offline_slack());
    statuses.insert("opencode".into(), opencode_status(&store));
    statuses.insert("t3".into(), offline_t3(&store));
    app.state::<ProviderCache>().replace(statuses);
}

async fn guarded<F: Future<Output = ProviderStatus>>(
    fallback: &'static str,
    future: F,
) -> ProviderStatus {
    match tokio::time::timeout(CHECK_TIMEOUT, future).await {
        Ok(status) => status,
        Err(_) => unavailable(fallback),
    }
}

fn offline_github(_store: &V2Store) -> ProviderStatus {
    match secrets::v2::read(GITHUB_KEY) {
        Some(token) => ProviderStatus {
            source: ProviderSource::Relay,
            status: ProviderState::ConfiguredUnverified,
            reason: Some("Stored GitHub token. Refresh to verify it.".into()),
            last_four: last_four(&token),
        },
        None => ProviderStatus {
            source: ProviderSource::None,
            status: ProviderState::MissingSetup,
            reason: Some("Add a GitHub token or sign in with gh.".into()),
            last_four: None,
        },
    }
}

async fn github_status(store: &V2Store) -> ProviderStatus {
    if let Some(token) = secrets::v2::read(GITHUB_KEY) {
        let mask = last_four(&token);
        let client = match GithubClient::new(Some(token)) {
            Ok(client) => client,
            Err(_) => {
                return ProviderStatus {
                    source: ProviderSource::Relay,
                    status: ProviderState::Unavailable,
                    reason: Some("GitHub rejected the stored token configuration.".into()),
                    last_four: mask,
                };
            }
        };
        let (status, reason) = match client.check().await {
            Ok(()) => (
                ProviderState::Verified,
                "Stored GitHub token verified.".to_string(),
            ),
            Err(_) => (
                ProviderState::Unavailable,
                "GitHub rejected the stored token. Update it to reconnect.".to_string(),
            ),
        };
        ProviderStatus {
            source: ProviderSource::Relay,
            status,
            reason: Some(reason),
            last_four: mask,
        }
    } else {
        // Reuse the gh session probe, but never v1 discovery or a v1 token.
        match github::session_login().await {
            Some(login) => ProviderStatus {
                source: ProviderSource::ExternalSession,
                status: ProviderState::Verified,
                reason: Some(format!("External gh session ({login}).")),
                last_four: None,
            },
            None => offline_github(store),
        }
    }
}

fn offline_clickup(store: &V2Store) -> ProviderStatus {
    let _ = store;
    match secrets::v2::read(CLICKUP_KEY) {
        Some(token) => ProviderStatus {
            source: ProviderSource::Relay,
            status: ProviderState::ConfiguredUnverified,
            reason: Some("Stored ClickUp token. Refresh to verify it.".into()),
            last_four: last_four(&token),
        },
        None => ProviderStatus {
            source: ProviderSource::None,
            status: ProviderState::MissingSetup,
            reason: Some("Add a ClickUp token for reference lookup and Import.".into()),
            last_four: None,
        },
    }
}

async fn clickup_status(store: &V2Store) -> ProviderStatus {
    let Some(token) = secrets::v2::read(CLICKUP_KEY) else {
        return offline_clickup(store);
    };
    let mask = last_four(&token);
    let client = match ClickUpClient::new(token) {
        Ok(client) => client,
        Err(_) => {
            return ProviderStatus {
                source: ProviderSource::Relay,
                status: ProviderState::Unavailable,
                reason: Some("ClickUp rejected the stored token configuration.".into()),
                last_four: mask,
            };
        }
    };
    let (status, reason) = match client.check().await {
        Ok(()) => (
            ProviderState::Verified,
            "Stored ClickUp token verified.".to_string(),
        ),
        Err(_) => (
            ProviderState::Unavailable,
            "ClickUp rejected the stored token. Update it to reconnect.".to_string(),
        ),
    };
    ProviderStatus {
        source: ProviderSource::Relay,
        status,
        reason: Some(reason),
        last_four: mask,
    }
}

fn offline_slack() -> ProviderStatus {
    if slack::find_claude().is_none() {
        ProviderStatus {
            source: ProviderSource::None,
            status: ProviderState::MissingSetup,
            reason: Some("Claude Code was not found on PATH.".into()),
            last_four: None,
        }
    } else {
        ProviderStatus {
            source: ProviderSource::ExternalSession,
            status: ProviderState::ConfiguredUnverified,
            reason: Some("Claude detected. Refresh to read its sign-in status.".into()),
            last_four: None,
        }
    }
}

/// Reads `claude auth status --json` with a short timeout. A signed-in Claude is not a
/// verified Slack connector: the connector results are not available to Relay yet.
async fn slack_status() -> ProviderStatus {
    let Some(program) = slack::find_claude() else {
        return offline_slack();
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(["auth", "status", "--json"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = match tokio::time::timeout(CHECK_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        _ => {
            return ProviderStatus {
                source: ProviderSource::ExternalSession,
                status: ProviderState::Unavailable,
                reason: Some("Claude sign-in status could not be read.".into()),
                last_four: None,
            };
        }
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let signed_in = text
        .find('{')
        .zip(text.rfind('}'))
        .and_then(|(start, end)| serde_json::from_str::<serde_json::Value>(&text[start..=end]).ok())
        .is_some_and(|value| {
            ["loggedIn", "isLoggedIn", "authenticated", "logged_in"]
                .iter()
                .any(|key| value.get(*key).and_then(|v| v.as_bool()) == Some(true))
        });
    if !output.status.success() || !signed_in {
        return ProviderStatus {
            source: ProviderSource::ExternalSession,
            status: ProviderState::MissingSetup,
            reason: Some("Claude Code is not signed in.".into()),
            last_four: None,
        };
    }
    ProviderStatus {
        source: ProviderSource::ExternalSession,
        status: ProviderState::ConfiguredUnverified,
        reason: Some(
            "Claude is signed in. Enable the Slack connector in Claude Code; Relay cannot verify connector results yet."
                .into(),
        ),
        last_four: None,
    }
}

fn opencode_status(store: &V2Store) -> ProviderStatus {
    let settings = match store.settings() {
        Ok(settings) => settings,
        Err(_) => return unavailable("OpenCode settings could not be read."),
    };
    let key = secrets::v2::read(OPENCODE_KEY);
    match settings.opencode_model {
        None => ProviderStatus {
            source: ProviderSource::None,
            status: ProviderState::MissingSetup,
            reason: Some("Set an OpenCode provider/model before briefing Tasks.".into()),
            last_four: key.as_deref().and_then(last_four),
        },
        Some(model) => ProviderStatus {
            source: ProviderSource::Relay,
            status: ProviderState::ConfiguredUnverified,
            reason: Some(format!(
                "Model {model}. {}",
                if key.is_some() {
                    "Provider key stored."
                } else {
                    "No provider key stored."
                }
            )),
            last_four: key.as_deref().and_then(last_four),
        },
    }
}

fn offline_t3(store: &V2Store) -> ProviderStatus {
    let mapped = store
        .list_projects()
        .is_ok_and(|projects| projects.iter().any(|p| p.t3_target.is_some()));
    if t3::find_launcher().is_none() {
        ProviderStatus {
            source: ProviderSource::Cli,
            status: ProviderState::MissingSetup,
            reason: Some("t3 was not found on PATH. Install T3 Code or restart Relay.".into()),
            last_four: None,
        }
    } else if mapped {
        ProviderStatus {
            source: ProviderSource::Cli,
            status: ProviderState::ConfiguredUnverified,
            reason: Some("T3 launcher found. Check a project mapping to verify it.".into()),
            last_four: None,
        }
    } else {
        ProviderStatus {
            source: ProviderSource::Cli,
            status: ProviderState::MissingSetup,
            reason: Some("Map a Relay project to a T3 project id or path.".into()),
            last_four: None,
        }
    }
}

/// Refreshes every section independently. One provider's timeout leaves the others intact.
pub async fn refresh_all(app: &AppHandle, store: &Arc<V2Store>) -> ProviderStatusMap {
    let mut statuses = ProviderStatusMap::new();
    statuses.insert(
        "github".into(),
        guarded("GitHub status timed out.", github_status(store)).await,
    );
    statuses.insert(
        "clickup".into(),
        guarded("ClickUp status timed out.", clickup_status(store)).await,
    );
    statuses.insert(
        "slack".into(),
        guarded("Claude sign-in status timed out.", slack_status()).await,
    );
    statuses.insert("opencode".into(), opencode_status(store));
    statuses.insert("t3".into(), offline_t3(store));
    app.state::<ProviderCache>().replace(statuses.clone());
    let _ = app.emit(PROVIDER_STATUS_EVENT, &statuses);
    statuses
}

fn publish_one(app: &AppHandle, key: &str, status: ProviderStatus) -> ProviderStatus {
    app.state::<ProviderCache>().put(key, status.clone());
    let _ = app.emit(
        PROVIDER_STATUS_EVENT,
        app.state::<ProviderCache>().snapshot(),
    );
    status
}

fn publish_change(app: &AppHandle) {
    let _ = app.emit(SETTINGS_CHANGE_EVENT, ());
}

/// A stable, one-way digest of a credential. It detects a rotated or removed token without
/// putting the token (or even a last-four mask) into a recovery row.
fn credential_digest(value: Option<&str>) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    if let Some(value) = value {
        for byte in value.trim().bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// A secret-free fingerprint of a provider's current credential configuration. Recovery pauses
/// record this value; a relevant change unpauses the work. A rotated `gh` session changes the
/// digest even when the login name stays the same.
pub async fn credential_fingerprint(provider: &str) -> String {
    match provider {
        "github" => {
            let stored = credential_digest(secrets::v2::read(GITHUB_KEY).as_deref());
            let session = credential_digest(github::session_token().await.as_deref());
            format!("v2:{stored}|gh:{session}")
        }
        "clickup" => format!(
            "v2:{}",
            credential_digest(secrets::v2::read(CLICKUP_KEY).as_deref())
        ),
        other => other.to_string(),
    }
}

/// A stored-token credential check. No polling, discovery or reanalysis is started.
#[tauri::command]
pub async fn v2_github_set_token(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
    token: String,
) -> Result<ProviderStatus, CommandError> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(CommandError::new(
            "invalid_input",
            "The GitHub token is empty.",
        ));
    }
    let client = GithubClient::new(Some(token.clone()))
        .map_err(|_| CommandError::new("invalid_input", "The GitHub token is invalid."))?;
    client.check().await.map_err(|_| {
        CommandError::new(
            "provider_rejected",
            "GitHub rejected the token. Check it and try again.",
        )
    })?;
    secrets::v2::write(GITHUB_KEY, &token)
        .map_err(|_| CommandError::new("keyring", "The token could not be stored."))?;
    let status = github_status(&store).await;
    Ok(publish_one(&app, "github", status))
}

#[tauri::command]
pub async fn v2_github_clear_token(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<ProviderStatus, CommandError> {
    secrets::v2::delete(GITHUB_KEY);
    let status = github_status(&store).await;
    Ok(publish_one(&app, "github", status))
}

#[tauri::command]
pub async fn v2_clickup_set_token(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
    token: String,
) -> Result<ProviderStatus, CommandError> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(CommandError::new(
            "invalid_input",
            "The ClickUp token is empty.",
        ));
    }
    let client = ClickUpClient::new(token.clone())
        .map_err(|_| CommandError::new("invalid_input", "The ClickUp token is invalid."))?;
    client.check().await.map_err(|_| {
        CommandError::new(
            "provider_rejected",
            "ClickUp rejected the token. Check it and try again.",
        )
    })?;
    secrets::v2::write(CLICKUP_KEY, &token)
        .map_err(|_| CommandError::new("keyring", "The token could not be stored."))?;
    let status = clickup_status(&store).await;
    Ok(publish_one(&app, "clickup", status))
}

#[tauri::command]
pub async fn v2_clickup_clear_token(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<ProviderStatus, CommandError> {
    secrets::v2::delete(CLICKUP_KEY);
    let status = clickup_status(&store).await;
    Ok(publish_one(&app, "clickup", status))
}

#[tauri::command]
pub async fn v2_clickup_workspaces(
    store: State<'_, Arc<V2Store>>,
) -> Result<Vec<relay_core::clickup::Named>, CommandError> {
    let token = secrets::v2::read(CLICKUP_KEY)
        .ok_or_else(|| CommandError::new("missing_setup", "Add a ClickUp token first."))?;
    let _ = &store;
    ClickUpClient::new(token)
        .map_err(|_| CommandError::new("invalid_input", "The stored ClickUp token is invalid."))?
        .workspaces()
        .await
        .map_err(|_| CommandError::new("provider_failed", "ClickUp workspaces could not be read."))
}

#[tauri::command]
pub async fn v2_clickup_spaces(
    workspace_id: String,
) -> Result<Vec<relay_core::clickup::Named>, CommandError> {
    let token = secrets::v2::read(CLICKUP_KEY)
        .ok_or_else(|| CommandError::new("missing_setup", "Add a ClickUp token first."))?;
    ClickUpClient::new(token)
        .map_err(|_| CommandError::new("invalid_input", "The stored ClickUp token is invalid."))?
        .spaces(workspace_id.trim())
        .await
        .map_err(|_| CommandError::new("provider_failed", "ClickUp spaces could not be read."))
}

#[tauri::command]
pub fn v2_provider_status(cache: State<'_, ProviderCache>) -> ProviderStatusMap {
    cache.snapshot()
}

#[tauri::command]
pub async fn v2_provider_refresh(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
) -> Result<ProviderStatusMap, CommandError> {
    Ok(refresh_all(&app, &store).await)
}

#[tauri::command]
pub fn v2_settings_update(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
    update: V2SettingsUpdate,
) -> Result<V2Settings, CommandError> {
    if let Some(raw) = &update.import_scope {
        let scope = V2ImportScope::from_json(raw)
            .ok_or_else(|| CommandError::new("invalid_input", "The Import scope is malformed."))?;
        let exists = store
            .list_projects()?
            .iter()
            .any(|project| project.id == scope.project_id);
        if !exists {
            return Err(CommandError::new(
                "invalid_input",
                "The Import scope names an unknown Relay project.",
            ));
        }
    }
    let settings = store.update_settings(update)?;
    publish_change(&app);
    Ok(settings)
}

#[tauri::command]
pub fn v2_opencode_set_model(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
    model: String,
) -> Result<V2Settings, CommandError> {
    let model = model.trim().to_string();
    if model
        .split_once('/')
        .is_none_or(|(provider, name)| provider.trim().is_empty() || name.trim().is_empty())
    {
        return Err(CommandError::new(
            "invalid_input",
            "Enter a provider/model, for example opencode/gpt-5-nano.",
        ));
    }
    let settings = store.set_opencode_model(Some(model))?;
    publish_one(&app, "opencode", opencode_status(&store));
    publish_change(&app);
    Ok(settings)
}

#[tauri::command]
pub fn v2_opencode_set_key(app: AppHandle, key: String) -> Result<ProviderStatus, CommandError> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(CommandError::new(
            "invalid_input",
            "The provider key is empty.",
        ));
    }
    secrets::v2::write(OPENCODE_KEY, &key)
        .map_err(|_| CommandError::new("keyring", "The key could not be stored."))?;
    let store = app.state::<Arc<V2Store>>();
    Ok(publish_one(&app, "opencode", opencode_status(&store)))
}

#[tauri::command]
pub fn v2_opencode_clear_key(app: AppHandle) -> ProviderStatus {
    secrets::v2::delete(OPENCODE_KEY);
    let store = app.state::<Arc<V2Store>>();
    publish_one(&app, "opencode", opencode_status(&store))
}

/// Read-only GitHub origin discovery, annotated against v2 projects only.
#[tauri::command]
pub async fn v2_repos_scan(
    store: State<'_, Arc<V2Store>>,
    folders: Vec<String>,
) -> Result<Vec<RepoCandidate>, CommandError> {
    let projects = store.list_projects()?;
    tauri::async_runtime::spawn_blocking(move || {
        let roots: Vec<&Path> = folders
            .iter()
            .map(|folder| Path::new(folder.trim()))
            .filter(|root| !root.as_os_str().is_empty())
            .collect();
        for root in &roots {
            if !root.is_dir() {
                return Err(CommandError::new(
                    "invalid_input",
                    format!("{} is not a folder", root.display()),
                ));
            }
        }
        let found = repos::scan_folders(&roots)
            .map_err(|error| CommandError::new("scan_failed", error.to_string()))?;
        Ok(found
            .into_iter()
            .map(|mut candidate| {
                candidate.project_id = projects
                    .iter()
                    .find(|project| {
                        project
                            .repo_path
                            .as_deref()
                            .is_some_and(|path| same_path(path, &candidate.path))
                    })
                    .map(|project| project.id.clone());
                candidate
            })
            .collect())
    })
    .await
    .map_err(|error| CommandError::new("scan_failed", error.to_string()))?
}

fn same_path(a: &str, b: &str) -> bool {
    fn normal(path: &str) -> String {
        path.trim()
            .trim_start_matches(r"\\?\")
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    }
    normal(a) == normal(b)
}

#[tauri::command]
pub fn v2_repos_register(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
    path: String,
    owner: String,
    repo: String,
) -> Result<V2Project, CommandError> {
    let path = path.trim();
    if !Path::new(path).is_dir() {
        return Err(CommandError::new(
            "invalid_input",
            "The repo folder does not exist.",
        ));
    }
    let owner = owner.trim();
    let name = repo.trim();
    if owner.is_empty() || name.is_empty() || owner.contains('/') || name.contains('/') {
        return Err(CommandError::new(
            "invalid_input",
            "A repo must have an owner and a name.",
        ));
    }
    let taken: HashSet<String> = store
        .list_projects()?
        .into_iter()
        .map(|project| project.slug)
        .collect();
    let base: String = name
        .chars()
        .map(|ch| ch.to_ascii_lowercase())
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    let base = if base.is_empty() {
        "repo".to_string()
    } else {
        base
    };
    let slug = (0..)
        .map(|n| {
            if n == 0 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|candidate| !taken.contains(candidate))
        .expect("an unbounded range always yields a free slug");
    let project = store.register_project(NewV2Project {
        name: name.to_string(),
        slug,
        repo_path: Some(path.to_string()),
        owner_repo: Some(format!("{owner}/{name}").to_lowercase()),
        t3_target: None,
    })?;
    publish_change(&app);
    Ok(project)
}

#[tauri::command]
pub fn v2_t3_set_target(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
    project_id: String,
    target: Option<T3TargetInput>,
) -> Result<V2Project, CommandError> {
    let target = match target {
        Some(input) => Some(T3Target::parse(&input.kind, &input.value)?),
        None => None,
    };
    let project = store.set_t3_target(&project_id, target)?;
    app.state::<ProviderCache>().put("t3", offline_t3(&store));
    let _ = app.emit(
        PROVIDER_STATUS_EVENT,
        app.state::<ProviderCache>().snapshot(),
    );
    publish_change(&app);
    Ok(project)
}

#[tauri::command]
pub async fn v2_t3_check_target(
    app: AppHandle,
    store: State<'_, Arc<V2Store>>,
    project_id: String,
) -> Result<T3MappingStatus, CommandError> {
    let target = store.t3_target(&project_id)?;
    let Some(target) = target else {
        return Ok(T3MappingStatus {
            status: ProviderState::MissingSetup,
            reason: "No T3 mapping is saved for this project.",
        });
    };
    let result = t3::check_target(&target).await;
    let status = match result.state {
        t3::MappingState::Ok => ProviderState::Verified,
        t3::MappingState::LauncherMissing | t3::MappingState::UnregisteredTarget => {
            ProviderState::MissingSetup
        }
        _ => ProviderState::Unavailable,
    };
    app.state::<ProviderCache>().put(
        "t3",
        ProviderStatus {
            source: ProviderSource::Cli,
            status,
            reason: Some(result.reason.into()),
            last_four: None,
        },
    );
    let _ = app.emit(
        PROVIDER_STATUS_EVENT,
        app.state::<ProviderCache>().snapshot(),
    );
    Ok(T3MappingStatus {
        status,
        reason: result.reason,
    })
}

/// Backend-only typed read for #36 and other Rust adapters. Not a Tauri command.
#[allow(dead_code)]
pub fn t3_target(store: &V2Store, project_id: &str) -> Result<Option<T3Target>, CommandError> {
    Ok(store.t3_target(project_id)?)
}

/// Backend-only OpenCode accessor for #34. Not a Tauri command and never serialized.
#[allow(dead_code)]
pub fn opencode_config(store: &V2Store) -> Result<OpencodeConfig, CommandError> {
    Ok(OpencodeConfig {
        model: store.settings()?.opencode_model,
        key: secrets::v2::read(OPENCODE_KEY),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_dto_carries_only_source_status_reason_and_a_mask() {
        let secret = "ghp_synthetic_value_1234";
        let status = ProviderStatus {
            source: ProviderSource::Relay,
            status: ProviderState::ConfiguredUnverified,
            reason: Some("Stored GitHub token. Refresh to verify it.".into()),
            last_four: last_four(secret),
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains(secret));
        assert!(!json.contains("ghp_"));
        assert!(json.contains("••••1234"));
        assert_eq!(status.last_four.as_deref(), Some("••••1234"));
        assert!(json.contains("configured_unverified"));
        assert!(json.contains("lastFour"));
    }

    #[test]
    fn an_empty_credential_has_no_mask() {
        assert_eq!(last_four(""), None);
        assert_eq!(last_four("   "), None);
        assert_eq!(last_four("abc").as_deref(), Some("••••abc"));
    }

    #[test]
    fn a_credential_digest_detects_rotation_without_leaking_the_value() {
        let secret = "ghp_synthetic_value_1234";
        let digest = credential_digest(Some(secret));
        assert!(!digest.contains("ghp_"));
        assert!(!digest.contains("1234"));
        assert_ne!(digest, credential_digest(Some("ghp_synthetic_value_5678")));
        assert_ne!(digest, credential_digest(None));
        assert_eq!(
            digest,
            credential_digest(Some(secret)),
            "stable across ticks"
        );
    }
}
