# Notes, slice 2: the Notes view, Send to T3, Turn into task

**Status:** final
**Agent / model:** Claude Opus 5.5
**Commit(s):** branch `feat/notes-view`, on top of slice 1 (`docs/reviews/2026-11-18-notes-store.md`)
**Files:** `src/app/Notes.tsx`, `src/notes.ts`, `src/notes.test.ts`, `src/app/Rail.tsx`, `src/route.ts`, `src/App.tsx`,
`src/ipc.ts`, `src/styles/app.css`

## What changed

- A **Notes** rail item and `/notes` route (`?view=notes` survives a reload).
- `NotesView`: a list (pinned first, then most recent) with search-as-you-type (150 ms debounce, FTS5 prefix
  search in Rust), and an editor (title + body). Edits autosave 600 ms after the last keystroke. A pending edit
  is flushed when you switch notes, leave the view, or reload. Saves are chained, so an older text never lands
  last. A status line shows Saving… / Saved / Not saved.
- Pin/unpin, and Delete with an in-app inline confirm (no native dialog).
- **Send to T3**: a Base UI `Select` picks a template (plain, `/ship`, `/orchestrate`, `/draft-issues`).
  `composePrompt` builds `<template> <title>\n\n<body>`, the view copies it with `navigator.clipboard`, then
  `open_t3` opens `t3code://`. Relay starts no agent; the user pastes the prompt into a thread.
- **Turn into task**: a Base UI `Select` picks the project, `note_to_task` creates the task (`source = note`), and
  the app emits `relay://change` and refreshes, so the task shows on Today and agents can claim it over MCP.
- All note IPC is `#[tauri::command(async)]` (slice 1), so nothing runs on the UI thread.

## Checks

`pnpm typecheck`, `pnpm test` (18 passed, 4 new for `composePrompt`, `noteLabel`, `sortNotes`) and `pnpm build`
pass. Rust is unchanged in this slice; `cargo test -p relay-core` passed on slice 1 (130 passed).

## Not covered / risks

- Not clicked through in the running Tauri app: the repo has no project verify skill. Clipboard access and the
  `t3code://` open were not exercised end to end.
- A search reload while a save is in flight shows the last saved label for that row until the save lands.
- The template list is fixed in code.

## Checklist

- [x] No native dialogs; Base UI `Select` for the template and project pickers
- [x] Work off the UI thread (async commands)
- [x] Relay never launches an agent
- [x] No new dependency

## Opus verdict
