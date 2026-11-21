//! Relay v2's fresh store. It owns its own file, schema and version, and shares no
//! table with the v1 [`crate::store::Store`]. No data migration from v1 exists.

pub mod composer;
pub mod live;
mod model;
pub mod recovery;
pub mod reference;
mod store;
#[cfg(test)]
mod tests;

pub use composer::{ComposerLineInput, ComposerSaveRequest, ParsedLine};
pub use live::{
    LiveCredentials, LiveQueryResult, ProviderProgress, ProviderProgressState, exact_clickup,
    exact_github, live_eligible, live_query, read_exact,
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
    ReferenceAvailability, ReferenceKey, ReferenceParseResult, ReferenceSnapshot, UrlMatch,
    V2PickerGroup, V2PickerRow, known_key, parse_urls, snapshot_needs_recovery,
};
pub use store::{V2Error, V2Store};
