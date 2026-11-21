//! Relay v2's fresh store. It owns its own file, schema and version, and shares no
//! table with the v1 [`crate::store::Store`]. No data migration from v1 exists.

mod model;
mod store;
#[cfg(test)]
mod tests;

pub use model::{
    AgentMode, ImportRecord, NewImportRecord, NewV2Project, NoteWrite, PreparedLine,
    PreparedNoteWrite, PreparedReference, PreparedTaskIntent, SavedNote, TaskStatus, V2Note,
    V2NoteLine, V2NotebookSnapshot, V2Project, V2Reference, V2Settings, V2SettingsUpdate, V2Task,
};
pub use store::{V2Error, V2Store};
