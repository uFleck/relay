//! Relay v2's fresh store. It owns its own file, schema and version, and shares no
//! table with the v1 [`crate::store::Store`]. No data migration from v1 exists.

pub mod composer;
pub mod import;
pub mod live;
mod model;
pub mod recovery;
pub mod reference;
pub mod slack_directory;
pub mod slack_thread;
pub mod slack_window;
mod store;
#[cfg(test)]
mod tests;

pub use composer::{ComposerLineInput, ComposerSaveRequest, ParsedLine};
pub use import::{
    CLICKUP_REFRESH_MS, GITHUB_REFRESH_MS, ImportCandidate, ImportOutcome, ImportRefreshTarget,
    ImportedState, RefreshUpdate, clickup_imported_state, dedupe_candidates, github_imported_state,
    refresh_failure_update, refresh_snapshot, refresh_success_update,
};
pub use live::{
    ExactItem, LiveCredentials, LiveQueryResult, ProviderProgress, ProviderProgressState,
    exact_clickup, exact_github, live_eligible, live_query, read_exact, read_exact_item,
};
pub use model::{
    AgentMode, GrillTurn, ImportRecord, NewImportRecord, NewV2Project, NoteWrite, PreparedLine,
    PreparedNoteWrite, PreparedReference, PreparedTaskIntent, SavedNote, T3Target,
    TaskProgressUpdate, TaskStatus, V2ImportScope, V2Note, V2NoteLine, V2NotebookSnapshot,
    V2Project, V2Reference, V2Settings, V2SettingsUpdate, V2Task, V2WorkspaceSnapshot,
    WorkspaceSearchResult,
};
pub use recovery::{
    MAX_IN_FLIGHT, NewRecovery, RETRY_SCHEDULE_MS, RecoveryReason, RecoveryTarget, RecoveryUpdate,
    STALE_SNAPSHOT_MS, decide_failure, decide_success, next_delay_ms,
};
pub use reference::{
    GithubItemKind, ReferenceAvailability, ReferenceKey, ReferenceParseResult, ReferenceSnapshot,
    UrlMatch, V2PickerGroup, V2PickerRow, known_key, parse_urls, snapshot_needs_recovery,
};
pub use slack_directory::{
    ConversationKind, DIRECTORY_TIMEOUT, DetailedHit, DirectoryConversation, DirectoryScanResult,
    DirectoryState, MAX_RUNS, SEARCH_TIMEOUT, STALE_DAYS, SlackWindow, directory_request,
    discovery, merge_conversations, note_discovery, parse_directory, parse_directory_run,
    parse_search_detailed, scan_directory, search_messages,
};
pub use slack_thread::{
    CachedMessage, ParsedThread, SlackThread, THREAD_TIMEOUT, ThreadFailure, ThreadResolution,
    parse_thread_full, resolve_thread, thread_request,
};
pub use slack_window::{
    WINDOW_TIMEOUT, WindowFailure, WindowResolution, bounds as window_bounds, failed_window,
    freeze_window, resolve_window,
};
pub use store::{DirectoryGuard, ThreadGuard, V2Error, V2Store};
