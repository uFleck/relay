//! Relay v2's fresh store. It owns its own file, schema and version, and shares no
//! table with the v1 [`crate::store::Store`]. No data migration from v1 exists.

pub mod composer;
mod model;
pub mod reference;
mod store;
#[cfg(test)]
mod tests;

pub use composer::{ComposerLineInput, ComposerSaveRequest, ParsedLine};
pub use model::{
    AgentMode, GrillTurn, ImportRecord, NewImportRecord, NewV2Project, NoteWrite, PreparedLine,
    PreparedNoteWrite, PreparedReference, PreparedTaskIntent, SavedNote, T3Target,
    TaskProgressUpdate, TaskStatus, V2ImportScope, V2Note, V2NoteLine, V2NotebookSnapshot,
    V2Project, V2Reference, V2Settings, V2SettingsUpdate, V2Task, V2WorkspaceSnapshot,
    WorkspaceSearchResult,
};
pub use reference::{
    ReferenceAvailability, ReferenceKey, ReferenceParseResult, ReferenceSnapshot, UrlMatch,
    V2PickerGroup, V2PickerRow, known_key, parse_urls,
};
pub use store::{V2Error, V2Store};
