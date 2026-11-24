# Workspace store per webview

Status: local implementation complete; CI pending.

- **Agent / model** — HungerG Free.
- **Commit(s)** — `7b22e4d` for the implementation. This ledger follows in a second commit.
- **Files** — `src/workspaceStore.ts`, `src/workspaceStore.test.ts`, `src/workspace.ts`, `src/hover/HoverApp.tsx`, `src/app/V2Capture.tsx`, this ledger.
- **What changed** — Added one Workspace store per webview for snapshot reads, event coalescing, request ordering and errors. Main window, Dock and the temporary capture pane now consume the same store through `useSyncExternalStore`.

## Commands + results

| Command | Result |
| --- | --- |
| `pnpm typecheck` | First run exposed a stale Notebook type and an unused unsubscribe field. Fixed both. Second run passed. |
| `pnpm test` | Pass, 47 tests. |
| `pnpm build` | Pass. |
| `cargo test -p relay-core` | Pass, 183 passed, 2 ignored. |
| `cargo check --workspace` | Pass. |
| `git diff --check` | Pass. |

The new store tests cover out-of-order responses, duplicate event coalescing and keeping the last workspace after an error.

## Known risks

- Task detail panels still fetch their own stage views.
- No live desktop run exercised cross-window refresh.
- The repository-wide format check remains red on pre-existing `keybinds.rs`.

## Review checklist

- [ ] Confirm one store instance exists per webview.
- [ ] Confirm older snapshot responses cannot replace newer state.
- [ ] Confirm duplicate change signals coalesce.
- [ ] Confirm the Dock badge still ignores active filters.

## Opus verdict
