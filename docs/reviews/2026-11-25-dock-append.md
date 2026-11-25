# Design polish 6/6: Dock Append

Written by Claude (Opus 5.5). Base: `master` after #77.

A Dock entry that starts with a note reference (`@N-…`) or a task reference (`@T-…`) is appended to that note, or to the task's note. The rest of the entry becomes new lines after the note's active lines, and its directives (`!`, `?`, `??`) create tasks there. Any other entry still creates a note. The main Notebook Composer is unchanged.

The rule lives in `relay-core`. `ComposerSaveRequest::Capture` resolves to the existing revision-guarded `Edit` when `append_target` finds a leading reference, and to `Create` otherwise, so append reuses the Edit path's task creation and conflict check. `V2Store` gains `note(id)` and `task(id)` readers. The Dock's Composer sends `kind: "capture"`, and its placeholder says how to append.

Files: `crates/relay-core/src/v2/{composer.rs,composer/tests.rs,store.rs}`, `src/components/Composer.tsx`, `src/ipc.ts`, `src/hover/HoverApp.tsx`.

Checks: `cargo test -p relay-core` (188 passed, 5 new), `cargo check -p relay`, `pnpm typecheck`, `pnpm test` (54 passed), `pnpm build`.

Risks: an append is an Edit with the revision read in the same call. A retried append after a lost response adds the lines twice, unlike Create, which is idempotent on its submission key. An empty append (reference only) and an unknown note are refused without writing. Not exercised in the real Dock window.

- [x] No native dialogs
- [x] No secrets reach the frontend
- [x] Policy in `relay-core`; the Tauri command is unchanged

## Opus verdict
