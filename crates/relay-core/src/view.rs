//! The main-window view model. These shapes mirror `src/types.ts` exactly and are the only
//! reason the frontend never sees a storage row.
//!
//! The store owns the data; this module owns the vocabulary translation:
//!
//! - **Projects**: `colorVar` is the project color when it is already a CSS variable
//!   (`--i3`), else `--i1..--i5` by project index.
//! - **Statuses**: canonicalized by status *name* across projects. The first project (the
//!   seeded `Inbox`) provides the canonical id for every name; names it does not have get
//!   a synthetic `status:<slug>` id. Columns sort by the UI category order, then the first
//!   project's position, then name. Categories map `backlog -> todo`; a `deploying`
//!   status whose name contains "waiting" becomes `waiting`.
//! - **Tasks**: `ref` is `RLY-` plus the last 6 ULID characters, uppercased. Two ids can
//!   share those 6 characters, so the ref is for humans; the id stays the key.
//! - **Due**: parsed RFC 3339; `hasTime` is false only for exactly `00:00:00`. Unparseable
//!   values keep `hasTime: true` because the store validates on write.
//! - **Claims**: only an unreleased, unexpired claim is exposed. The holder comes from
//!   `holder_name`/`holder_kind` (`opencode` -> OpenCode, `claude` -> Claude Code, else
//!   T3). `lastProgressAt` is the newest `progress` event, else `claimedAt`;
//!   `lastProgress` is that event's body, else `"claimed"`.
//! - **Changes**: active changes only, newest first. Requirements keep their source,
//!   author, confidence and permalink; every other kind is a field change whose `field`
//!   is the stored kind and whose `to` is the body.
//! - **Links**: storage kinds collapse onto the six UI kinds; `branch` and `commit` both
//!   open as `branch`.
//! - **Proposals**: empty in P1 (JEV is P3). The inbox count still includes pending
//!   proposals so the badge is right once they exist.

use std::{collections::HashMap, collections::HashSet};

use serde::{Deserialize, Serialize};

use crate::model;

/// Shared by the model and the UI: a claim lease is 30 minutes.
const LEASE_MINUTES: i64 = 30;
/// Circuit identity tokens, in project order.
const PALETTE: [&str; 5] = ["--i1", "--i2", "--i3", "--i4", "--i5"];

// ---------------------------------------------------------------------------
// Wire shapes (`src/types.ts`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub version: String,
    pub projects: Vec<Project>,
    pub statuses: Vec<Status>,
    pub tasks: Vec<Task>,
    pub proposals: Vec<Proposal>,
    pub sync: Vec<SyncSource>,
    pub update: UpdateState,
    pub inbox_count: usize,
    pub last_proposal_handled_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    /// The checkout the git/PR context reads; `null` when the project has none.
    pub repo_path: Option<String>,
    pub color_var: String,
}

/// The UI's eight category keys, which differ from the storage categories: `backlog` folds
/// into `todo` and `waiting` exists only in the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Todo,
    Active,
    Review,
    Merged,
    Waiting,
    Deploying,
    Blocked,
    Done,
}

impl Category {
    /// Column order in Today, the stepper and the group headers.
    fn order(self) -> u8 {
        match self {
            Self::Todo => 0,
            Self::Active => 1,
            Self::Review => 2,
            Self::Merged => 3,
            Self::Waiting => 4,
            Self::Deploying => 5,
            Self::Blocked => 6,
            Self::Done => 7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub id: String,
    pub name: String,
    /// Short label for the sheet stepper.
    pub short: String,
    pub category: Category,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaimHolder {
    #[serde(rename = "T3")]
    T3,
    #[serde(rename = "OpenCode")]
    OpenCode,
    #[serde(rename = "Claude Code")]
    ClaudeCode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    pub id: String,
    pub holder: ClaimHolder,
    pub claimed_at: String,
    pub last_progress_at: String,
    pub last_progress: String,
    pub lease_minutes: i64,
    pub released_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Slack,
    Github,
    Clickup,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequirementChange {
    pub id: String,
    pub at: String,
    pub source: Source,
    pub author: String,
    pub text: String,
    pub confidence: f64,
    pub permalink: String,
    pub acked_at: Option<String>,
    pub acked_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldChange {
    pub id: String,
    pub at: String,
    pub source: Source,
    pub field: String,
    pub from: String,
    pub to: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// Internally tagged on `kind`, matching the TypeScript union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Change {
    Requirement(RequirementChange),
    Field(FieldChange),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    Slack,
    Thread,
    Pr,
    Branch,
    Clickup,
    Github,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLink {
    pub kind: LinkKind,
    pub label: String,
    pub state: String,
    pub url: String,
    /// The polled metadata for a PR (`checks`, `review`, `headSha`), as JSON text.
    pub metadata_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Due {
    pub at: String,
    pub has_time: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    #[serde(rename = "ref")]
    pub r#ref: String,
    pub rev: i64,
    pub title: String,
    pub project_id: String,
    pub status_id: String,
    pub due: Option<Due>,
    pub claim: Option<Claim>,
    pub description: String,
    pub changes: Vec<Change>,
    pub links: Vec<TaskLink>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvoPart {
    pub text: String,
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub matches: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvoMessage {
    pub at: String,
    pub who: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub me: Option<bool>,
    pub parts: Vec<ConvoPart>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JevTone {
    Accent,
    Muted,
    Warn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JevScore {
    pub label: String,
    pub value: f64,
    pub threshold: f64,
    pub tone: JevTone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Jev {
    pub stage: String,
    pub ms: i64,
    pub scores: Vec<JevScore>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldDiff {
    pub field: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<Due>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requirement: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    ChangeDeadline,
    AddRequirement,
    NewTask,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: String,
    pub kind: ProposalKind,
    pub task_id: String,
    pub task_rev: i64,
    pub origin: String,
    pub author: String,
    pub episode: i64,
    pub messages: Vec<ConvoMessage>,
    pub jev: Jev,
    pub diff: Vec<FieldDiff>,
    /// What approving writes to the task; the UI applies it optimistically.
    pub patch: ProposalPatch,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncState {
    Ok,
    Retrying,
    Down,
    /// Not configured. Neutral, not an error.
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSource {
    pub name: String,
    pub short: String,
    pub state: SyncState,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum UpdateState {
    Idle {
        version: String,
        #[serde(rename = "checkedAt")]
        checked_at: String,
    },
    Ready {
        version: String,
        #[serde(rename = "sizeMb")]
        size_mb: f64,
        signed: bool,
        note: String,
    },
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

/// Raw rows for one snapshot read. The store fills this in one pass under its lock; the
/// mapping below is pure so it stays testable without SQLite.
pub(crate) struct SnapshotData {
    pub(crate) projects: Vec<model::Project>,
    pub(crate) statuses: Vec<model::ProjectStatus>,
    pub(crate) tasks: Vec<model::Task>,
    /// Active claims only; at most one per task.
    pub(crate) claims: Vec<model::Claim>,
    /// `progress` events, newest per task.
    pub(crate) progress: Vec<model::TaskEvent>,
    /// Active changes, oldest first.
    pub(crate) changes: Vec<model::TaskChange>,
    /// `(change_id, acking claim id)`, earliest ack first; `None` is a human ack.
    pub(crate) acks: Vec<(String, Option<String>)>,
    pub(crate) links: Vec<model::TaskLink>,
    pub(crate) pending_proposals: usize,
    pub(crate) last_proposal_handled_at: Option<String>,
}

pub(crate) fn build_snapshot(data: SnapshotData, version: &str, sync: Vec<SyncSource>) -> Snapshot {
    let SnapshotData {
        projects: project_rows,
        statuses: status_rows,
        tasks: task_rows,
        claims: claim_rows,
        progress: progress_rows,
        changes: change_rows,
        acks: ack_rows,
        links: link_rows,
        pending_proposals,
        last_proposal_handled_at,
    } = data;

    let projects: Vec<Project> = project_rows
        .iter()
        .enumerate()
        .map(|(index, project)| Project {
            id: project.id.clone(),
            name: project.name.clone(),
            repo_path: project.repo_path.clone(),
            color_var: color_var(project.color.as_deref(), index),
        })
        .collect();

    let (statuses, status_ids) = canonical_statuses(&project_rows, &status_rows);

    let mut claims: HashMap<String, model::Claim> = HashMap::new();
    for claim in claim_rows {
        let task_id = claim.task_id.clone();
        claims.entry(task_id).or_insert(claim);
    }

    let mut progress: HashMap<String, model::TaskEvent> = HashMap::new();
    for event in progress_rows {
        let task_id = event.task_id.clone();
        progress.entry(task_id).or_insert(event);
    }

    let mut changes: HashMap<String, Vec<model::TaskChange>> = HashMap::new();
    for change in change_rows {
        changes
            .entry(change.task_id.clone())
            .or_default()
            .push(change);
    }

    let mut acks: HashMap<String, Option<String>> = HashMap::new();
    for (change_id, claim_id) in ack_rows {
        acks.entry(change_id).or_insert(claim_id);
    }

    let mut links: HashMap<String, Vec<model::TaskLink>> = HashMap::new();
    for link in link_rows {
        links.entry(link.task_id.clone()).or_default().push(link);
    }

    // Human inbox: active requirements nobody acked, on the tasks this snapshot shows.
    // Computed before the task loop drains the per-task change lists.
    let visible: HashSet<&str> = task_rows.iter().map(|task| task.id.as_str()).collect();
    let unacked_requirements = changes
        .iter()
        .filter(|(task_id, _)| visible.contains(task_id.as_str()))
        .flat_map(|(_, task_changes)| task_changes)
        .filter(|change| change.kind == model::ChangeKind::Requirement && change.acked_at.is_none())
        .count();

    let mut tasks = Vec::with_capacity(task_rows.len());
    for task in &task_rows {
        let mut task_changes = changes.remove(&task.id).unwrap_or_default();
        // The store returns changes oldest first; the timeline reads newest first.
        task_changes.reverse();
        let changes = task_changes
            .iter()
            .map(|change| change_view(change, &acks))
            .collect();

        let claim = claims
            .get(&task.id)
            .map(|claim| claim_view(claim, progress.get(&task.id)));

        tasks.push(Task {
            id: task.id.clone(),
            r#ref: task_ref(&task.id),
            rev: task.rev,
            title: task.title.clone(),
            project_id: task.project_id.clone(),
            status_id: status_ids
                .get(&task.status_id)
                .cloned()
                .unwrap_or_else(|| task.status_id.clone()),
            due: due_view(task.due_at.as_deref()),
            claim,
            description: task.notes.clone(),
            changes,
            links: links
                .remove(&task.id)
                .unwrap_or_default()
                .iter()
                .map(link_view)
                .collect(),
        });
    }

    Snapshot {
        version: version.to_string(),
        projects,
        statuses,
        tasks,
        // JEV proposals are P3; the queue stays empty instead of inventing rows.
        proposals: Vec::new(),
        sync,
        update: UpdateState::Idle {
            version: version.to_string(),
            checked_at: model::now(),
        },
        inbox_count: unacked_requirements + pending_proposals,
        last_proposal_handled_at,
    }
}

fn color_var(color: Option<&str>, index: usize) -> String {
    match color.map(str::trim).filter(|value| value.starts_with("--")) {
        Some(value) => value.to_string(),
        None => PALETTE[index % PALETTE.len()].to_string(),
    }
}

/// Canonical status columns plus a raw status id -> canonical id map.
fn canonical_statuses(
    projects: &[model::Project],
    rows: &[model::ProjectStatus],
) -> (Vec<Status>, HashMap<String, String>) {
    let baseline: HashMap<&str, &model::ProjectStatus> = match projects.first() {
        Some(first) => rows
            .iter()
            .filter(|row| row.project_id == first.id)
            .map(|row| (row.name.trim(), row))
            .collect(),
        None => HashMap::new(),
    };

    let mut status_ids = HashMap::new();
    let mut seen = HashSet::new();
    let mut built: Vec<(Status, i64)> = Vec::new();

    for row in rows {
        let name = row.name.trim();
        let (id, display, category, position) = match baseline.get(name) {
            Some(first) => (
                first.id.clone(),
                first.name.trim().to_string(),
                category_for(name, first.category),
                first.position,
            ),
            None => (
                synthetic_status_id(name),
                name.to_string(),
                category_for(name, row.category),
                row.position,
            ),
        };

        status_ids.insert(row.id.clone(), id.clone());
        if seen.insert(id.clone()) {
            built.push((
                Status {
                    id,
                    name: display,
                    short: short_label(name),
                    category,
                },
                position,
            ));
        }
    }

    built.sort_by(|(a, position_a), (b, position_b)| {
        a.category
            .order()
            .cmp(&b.category.order())
            .then(position_a.cmp(position_b))
            .then(a.name.cmp(&b.name))
    });
    (
        built.into_iter().map(|(status, _)| status).collect(),
        status_ids,
    )
}

fn category_for(name: &str, category: model::StatusCategory) -> Category {
    match category {
        model::StatusCategory::Backlog | model::StatusCategory::Todo => Category::Todo,
        model::StatusCategory::Active => Category::Active,
        model::StatusCategory::Review => Category::Review,
        model::StatusCategory::Merged => Category::Merged,
        model::StatusCategory::Deploying if name.to_lowercase().contains("waiting") => {
            Category::Waiting
        }
        model::StatusCategory::Deploying => Category::Deploying,
        model::StatusCategory::Blocked => Category::Blocked,
        model::StatusCategory::Done => Category::Done,
    }
}

/// The stepper label. The default workflow is spelled out; a custom status falls back to
/// its first word so a long name never overflows the stepper.
fn short_label(name: &str) -> String {
    match name {
        "In Progress" => "Doing".to_string(),
        "In Review" => "Review".to_string(),
        "Waiting for deploy" => "Deploy".to_string(),
        "Deployed to RC" => "RC".to_string(),
        other => other.split_whitespace().next().unwrap_or(other).to_string(),
    }
}

/// `status:<slug>`; two names that slug the same way share a column (documented risk).
fn synthetic_status_id(name: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.extend(character.to_lowercase());
            dash = false;
        } else if !dash && !slug.is_empty() {
            slug.push('-');
            dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    format!(
        "status:{}",
        if slug.is_empty() {
            "unknown"
        } else {
            slug.as_str()
        }
    )
}

fn task_ref(id: &str) -> String {
    let mut tail: Vec<char> = id.chars().rev().take(6).collect();
    tail.reverse();
    format!(
        "RLY-{}",
        tail.into_iter().collect::<String>().to_uppercase()
    )
}

fn due_view(value: Option<&str>) -> Option<Due> {
    let at = value?;
    let has_time = model::parse_time(at)
        .is_none_or(|parsed| parsed.hour() != 0 || parsed.minute() != 0 || parsed.second() != 0);
    Some(Due {
        at: at.to_string(),
        has_time,
    })
}

fn claim_view(claim: &model::Claim, progress: Option<&model::TaskEvent>) -> Claim {
    let (last_progress_at, last_progress) = match progress {
        Some(event) => (
            event.created_at.clone(),
            event.body.clone().unwrap_or_else(|| "claimed".to_string()),
        ),
        None => (claim.created_at.clone(), "claimed".to_string()),
    };
    Claim {
        id: claim.id.clone(),
        holder: holder(claim),
        claimed_at: claim.created_at.clone(),
        last_progress_at,
        last_progress,
        lease_minutes: LEASE_MINUTES,
        released_at: claim.released_at.clone(),
    }
}

fn holder(claim: &model::Claim) -> ClaimHolder {
    let identity = format!("{} {}", claim.holder_name, claim.holder_kind).to_lowercase();
    if identity.contains("opencode") {
        ClaimHolder::OpenCode
    } else if identity.contains("claude") {
        ClaimHolder::ClaudeCode
    } else {
        ClaimHolder::T3
    }
}

fn source(source_kind: &str) -> Source {
    match source_kind.trim().to_lowercase().as_str() {
        "github" => Source::Github,
        "clickup" => Source::Clickup,
        "agent" => Source::Agent,
        // Slack is the capture path; unknown or human sources read as Slack.
        _ => Source::Slack,
    }
}

fn change_view(change: &model::TaskChange, acks: &HashMap<String, Option<String>>) -> Change {
    let acked_by = change.acked_at.as_ref().map(|_| {
        acks.get(&change.id)
            .and_then(|claim_id| claim_id.clone())
            .unwrap_or_else(|| "you".to_string())
    });

    match change.kind {
        model::ChangeKind::Requirement => Change::Requirement(RequirementChange {
            id: change.id.clone(),
            at: change.created_at.clone(),
            source: source(&change.source_kind),
            author: change
                .source_id
                .clone()
                .unwrap_or_else(|| change.source_kind.clone()),
            text: change.body.clone(),
            confidence: change.confidence.unwrap_or(0.0),
            permalink: change.permalink.clone().unwrap_or_default(),
            acked_at: change.acked_at.clone(),
            acked_by,
        }),
        _ => Change::Field(FieldChange {
            id: change.id.clone(),
            at: change.created_at.clone(),
            source: source(&change.source_kind),
            field: change.kind.as_str().to_string(),
            from: String::new(),
            to: change.body.clone(),
            reference: change
                .permalink
                .clone()
                .or_else(|| change.source_id.clone())
                .unwrap_or_default(),
        }),
    }
}

fn link_view(link: &model::TaskLink) -> TaskLink {
    TaskLink {
        kind: link_kind(&link.kind),
        label: link.title.clone().unwrap_or_else(|| link.kind.clone()),
        state: link.state.clone().unwrap_or_default(),
        url: link.url.clone(),
        metadata_json: (link.metadata_json.trim() != "{}" && !link.metadata_json.trim().is_empty())
            .then(|| link.metadata_json.clone()),
    }
}

fn link_kind(kind: &str) -> LinkKind {
    match kind.trim().to_lowercase().as_str() {
        "slack" => LinkKind::Slack,
        "pr" => LinkKind::Pr,
        "branch" | "commit" => LinkKind::Branch,
        "clickup" => LinkKind::Clickup,
        "github" => LinkKind::Github,
        // `t3`, `thread`, `url` and anything new all open like a thread.
        _ => LinkKind::Thread,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChangeKind, NewChange, NewLink, NewProject, NewTask, StatusCategory};
    use crate::store::Store;

    fn project_row(id: &str, name: &str) -> model::Project {
        model::Project {
            id: id.to_string(),
            name: name.to_string(),
            slug: name.to_lowercase(),
            repo_path: None,
            color: None,
            integrations_json: "{}".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn status_row(
        id: &str,
        project_id: &str,
        name: &str,
        category: StatusCategory,
        position: i64,
    ) -> model::ProjectStatus {
        model::ProjectStatus {
            id: id.to_string(),
            project_id: project_id.to_string(),
            name: name.to_string(),
            category,
            position,
            color: None,
        }
    }

    #[test]
    fn snapshot_maps_the_store_to_the_ui_contract() {
        let store = Store::open_in_memory().unwrap();
        let created = store
            .create_project_with_default_workflow(NewProject::new("Inbox", "inbox", None))
            .unwrap();
        let project = created.project;
        let statuses = created.statuses;
        let waiting = statuses
            .iter()
            .find(|status| status.name == "Waiting for deploy")
            .unwrap();

        let mut first = NewTask::new(&project.id, "Ship the snapshot");
        first.status_id = Some(waiting.id.clone());
        first.due_at = Some("2026-11-17T17:30:00.000Z".to_string());
        let task = store.create_task(first).unwrap();
        let second = store
            .create_task(NewTask {
                due_at: Some("2026-11-18T00:00:00.000Z".to_string()),
                ..NewTask::new(&project.id, "Second task")
            })
            .unwrap();

        let mut requirement = NewChange::requirement(&task.id, "Put rollback first", "slack");
        requirement.source_id = Some("U123".to_string());
        requirement.confidence = Some(0.83);
        requirement.permalink = Some("https://slack.example/p1".to_string());
        let acked = store.add_change(requirement).unwrap();

        let unacked = store
            .add_change(NewChange::requirement(
                &task.id,
                "Works on the RC build",
                "slack",
            ))
            .unwrap();

        let mut deadline = NewChange::requirement(&task.id, "2026-11-20", "github");
        deadline.kind = ChangeKind::ChangeDeadline;
        let deadline = store.add_change(deadline).unwrap();

        let mut thread = NewLink::new(&task.id, "t3", "t3://thread/1");
        thread.title = Some("T3 thread".to_string());
        store.add_link(thread).unwrap();
        let mut pr = NewLink::new(&task.id, "pr", "https://github.example/pull/1");
        pr.state = Some("open".to_string());
        store.add_link(pr).unwrap();

        let snapshot = store.ui_snapshot("0.4.1", Vec::new()).unwrap();

        assert_eq!(snapshot.version, "0.4.1");
        assert!(snapshot.proposals.is_empty());
        assert!(snapshot.sync.is_empty());
        assert!(matches!(snapshot.update, UpdateState::Idle { .. }));
        assert_eq!(snapshot.inbox_count, 2);
        assert!(snapshot.last_proposal_handled_at.is_none());

        let categories: Vec<Category> = snapshot.statuses.iter().map(|s| s.category).collect();
        assert_eq!(
            categories,
            vec![
                Category::Todo,
                Category::Todo,
                Category::Active,
                Category::Review,
                Category::Merged,
                Category::Waiting,
                Category::Deploying,
                Category::Blocked,
                Category::Done,
            ]
        );
        let deploy = snapshot
            .statuses
            .iter()
            .find(|status| status.name == "Waiting for deploy")
            .unwrap();
        assert_eq!(deploy.id, waiting.id);
        assert_eq!(deploy.category, Category::Waiting);
        assert_eq!(deploy.short, "Deploy");

        let view_task = snapshot
            .tasks
            .iter()
            .find(|candidate| candidate.id == task.id)
            .unwrap();
        assert_eq!(
            view_task.r#ref,
            format!("RLY-{}", task.id[task.id.len() - 6..].to_uppercase())
        );
        assert_eq!(view_task.rev, 1);
        assert_eq!(view_task.status_id, waiting.id);
        assert_eq!(view_task.description, "");
        let due = view_task.due.as_ref().unwrap();
        assert_eq!(due.at, "2026-11-17T17:30:00.000Z");
        assert!(due.has_time);

        let change_ids: Vec<&str> = view_task
            .changes
            .iter()
            .map(|change| match change {
                Change::Requirement(change) => change.id.as_str(),
                Change::Field(change) => change.id.as_str(),
            })
            .collect();
        assert_eq!(
            change_ids,
            vec![deadline.id.as_str(), unacked.id.as_str(), acked.id.as_str()]
        );

        let requirement = view_task
            .changes
            .iter()
            .find_map(|change| match change {
                Change::Requirement(change) if change.id == acked.id => Some(change),
                _ => None,
            })
            .unwrap();
        assert_eq!(requirement.source, Source::Slack);
        assert_eq!(requirement.author, "U123");
        assert_eq!(requirement.text, "Put rollback first");
        assert_eq!(requirement.confidence, 0.83);
        assert_eq!(requirement.permalink, "https://slack.example/p1");
        assert!(requirement.acked_at.is_none());
        assert!(requirement.acked_by.is_none());

        let field = view_task
            .changes
            .iter()
            .find_map(|change| match change {
                Change::Field(change) if change.id == deadline.id => Some(change),
                _ => None,
            })
            .unwrap();
        assert_eq!(field.field, "change_deadline");
        assert_eq!(field.from, "");
        assert_eq!(field.to, "2026-11-20");
        assert_eq!(field.reference, "");
        assert_eq!(field.source, Source::Github);

        assert_eq!(view_task.links.len(), 2);
        let thread = view_task
            .links
            .iter()
            .find(|link| link.kind == LinkKind::Thread)
            .unwrap();
        assert_eq!(thread.label, "T3 thread");
        assert_eq!(thread.state, "");
        let pr = view_task
            .links
            .iter()
            .find(|link| link.kind == LinkKind::Pr)
            .unwrap();
        assert_eq!(pr.label, "pr");
        assert_eq!(pr.state, "open");

        let view_second = snapshot
            .tasks
            .iter()
            .find(|candidate| candidate.id == second.id)
            .unwrap();
        assert!(view_second.claim.is_none());
        assert!(view_second.changes.is_empty());
        assert!(!view_second.due.as_ref().unwrap().has_time);
    }

    #[test]
    fn wire_json_uses_the_typescript_names() {
        let snapshot = Snapshot {
            version: "0.1.0".to_string(),
            projects: vec![Project {
                id: "p".to_string(),
                name: "Inbox".to_string(),
                repo_path: None,
                color_var: "--i1".to_string(),
            }],
            statuses: vec![Status {
                id: "s".to_string(),
                name: "Todo".to_string(),
                short: "Todo".to_string(),
                category: Category::Todo,
            }],
            tasks: Vec::new(),
            proposals: Vec::new(),
            sync: Vec::new(),
            update: UpdateState::Idle {
                version: "0.1.0".to_string(),
                checked_at: "2026-11-17T12:00:00.000Z".to_string(),
            },
            inbox_count: 0,
            last_proposal_handled_at: None,
        };
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["projects"][0]["colorVar"], "--i1");
        assert_eq!(json["inboxCount"], 0);
        assert_eq!(json["lastProposalHandledAt"], serde_json::Value::Null);
        assert_eq!(
            json["update"],
            serde_json::json!({
                "state": "idle",
                "version": "0.1.0",
                "checkedAt": "2026-11-17T12:00:00.000Z",
            })
        );

        let requirement = Change::Requirement(RequirementChange {
            id: "c".to_string(),
            at: "2026-11-17T12:00:00.000Z".to_string(),
            source: Source::Slack,
            author: "U1".to_string(),
            text: "do it".to_string(),
            confidence: 0.5,
            permalink: "https://example".to_string(),
            acked_at: None,
            acked_by: None,
        });
        let json = serde_json::to_value(requirement).unwrap();
        assert_eq!(json["kind"], "requirement");
        assert_eq!(json["ackedAt"], serde_json::Value::Null);
        assert_eq!(json["ackedBy"], serde_json::Value::Null);

        assert_eq!(
            serde_json::to_value(ClaimHolder::ClaudeCode).unwrap(),
            "Claude Code"
        );
    }

    #[test]
    fn statuses_are_canonical_across_projects() {
        let first = project_row("p1", "Inbox");
        let second = project_row("p2", "Project");
        let rows = vec![
            status_row(
                "s1",
                "p1",
                "Waiting for deploy",
                StatusCategory::Deploying,
                5,
            ),
            status_row(
                "s2",
                "p2",
                "Waiting for deploy",
                StatusCategory::Deploying,
                5,
            ),
            status_row("s3", "p2", "QA Review", StatusCategory::Review, 9),
        ];

        let (statuses, ids) = canonical_statuses(&[first, second], &rows);

        assert_eq!(ids["s1"], "s1");
        assert_eq!(ids["s2"], "s1");
        assert_eq!(ids["s3"], "status:qa-review");
        assert_eq!(statuses.len(), 2);
        let qa = statuses
            .iter()
            .find(|status| status.name == "QA Review")
            .unwrap();
        assert_eq!(qa.id, "status:qa-review");
        assert_eq!(qa.short, "QA");
        assert_eq!(qa.category, Category::Review);
    }

    #[test]
    fn project_colors_use_the_palette_unless_they_are_variables() {
        assert_eq!(color_var(Some("--i3"), 0), "--i3");
        assert_eq!(color_var(Some(" #ff0000 "), 0), "--i1");
        assert_eq!(color_var(None, 7), "--i3");
    }
}
