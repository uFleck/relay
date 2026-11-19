# Notes, slice 1: storage, service and IPC

**Status:** final
**Agent / model:** Claude Opus 5.5
**Commit(s):** branch `t3/notes-view-t3-handoff`, on top of `0a91ce2` (v0.7.0)
**Files:** `crates/relay-core/migrations/0005_notes.sql`, `crates/relay-core/src/store/notes.rs`,
`crates/relay-core/src/store/mod.rs`, `crates/relay-core/src/model.rs`, `src-tauri/src/commands.rs`,
`src-tauri/src/lib.rs`, `src/ipc.ts`

## What changed

- Migration 0005 adds `notes` (`seq` rowid alias, ULID `id`, `title`, `body`, `pinned`, RFC 3339
  `created_at` / `updated_at`) and an external-content FTS5 index `notes_fts` over title and body, kept in sync by
  triggers like `tasks_fts`. `seq` is an explicit `INTEGER PRIMARY KEY`, so FTS rowids survive a `VACUUM`.
- `store/notes.rs`: `create_note`, `get_note`, `update_note` (last write wins), `set_note_pinned` (keeps
  `updated_at`), `delete_note`, `list_notes` (pinned first, then most recent; a query goes through FTS5 with every
  token a quoted literal prefix) and `note_to_task` (title, or the body's first line, becomes the task title; the
  body becomes the task notes; `source = "note"`, `metadata.noteId` links back; the note stays).
- Tauri: `notes_list`, `note_create`, `note_update`, `note_set_pinned`, `note_delete`, `note_to_task` and `open_t3`,
  all `#[tauri::command(async)]` so a store-lock wait never blocks the window. `open_t3` opens `t3code://` from Rust
  (the protocol T3 Code registers on Windows), so no new opener scope is needed. It only opens the app.
- `src/ipc.ts`: typed wrappers with an in-memory browser fallback.

## Checks

`cargo test -p relay-core` (130 passed, 2 ignored), `cargo fmt --all --check`,
`cargo clippy -p relay-core -p relay --all-targets -- -D warnings`, `pnpm typecheck`, `pnpm test` (14 passed): all
pass. New tests: round trip + pin order, prefix search that follows edits and deletes with FTS operators kept
literal, note → task, and an upgrade from user_version 4 that keeps tasks and adds searchable notes.

## Risks

- No UI uses the commands yet (slice 2).
- `open_t3` assumes the `t3code://` protocol; when it is missing the command returns `open_failed`.

## Checklist

- [x] Policy in relay-core; IPC commands stay thin
- [x] Migration through `PRAGMA user_version`, with an upgrade test
- [x] No new dependency; no secrets touched
- [x] Commands off the UI thread

## Opus verdict
