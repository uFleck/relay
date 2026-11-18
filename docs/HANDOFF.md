# Relay handoff (for Opus)

State as of 2026-11-17 20:20 (-03:00). Read this before resuming the UI or starting the MCP boundary.

## Commits

| Commit | Owner | What |
| --- | --- | --- |
| `924aae1` | DeepSeek | Scaffold: Tauri shell, core model/store, release pipeline |
| `1e26ee3` | Opus | Hover window, tray, hotkey, updater store |
| `c4721f9` | DeepSeek | Core store, claims, changes, proposals (19 tests then; 23 now) |
| `70f8eae` | DeepSeek | Review ledger + release audit (fixed missing `--verify-tag`) |
| `91e4f0a` | DeepSeek | UI snapshot + IPC commands (`relay_snapshot`, `ack_change`, `decide_proposal`) |
| `6b2bad9` | DeepSeek | JEV System One client + routing thresholds (16 tests) |
| `70be4a4` | DeepSeek | Core regression tests + FTS escaping + claim `busy_timeout` |
| `674c2b9` | DeepSeek | `cargo fmt` on `hover.rs` (no behavior change) |

Uncommitted: the partial main UI (`src/app/**`, `src/components/**`, `src/data/**`, `src/ipc.ts`, `src/model.ts`, `src/types.ts`, `src/styles/app.css`, `src/styles/layout-tokens.css`, modified `src/App.tsx`, `src/main.tsx`, `src/styles/base.css`) and one unused-import fix in `src/components/bits.tsx`. `hover.html` and `src/hover/**` do not exist yet.

## Verified

- `cargo test -p relay-core`: 45 passed, 0 failed.
- `cargo clippy -p relay-core --all-targets -- -D warnings`: clean.
- `cargo check`: clean.
- `pnpm typecheck` and `pnpm build`: pass (after removing the unused `Icon` import).
- Signed release build: `pnpm tauri build` exited 0 in **268.3s (4m28s)**, produced:
  - `target/release/bundle/nsis/Relay_0.1.0_x64-setup.exe` (4.27 MiB)
  - `...exe.sig` (432 bytes)
- Runtime smoke test: the release exe starts, creates `%APPDATA%\dev.relay.app\relay.db` at `user_version 2`, seeds `Inbox` + 9 statuses.
- Visual smoke test with 5 seeded tasks: status groups, T3/OpenCode claims, unacked change dot, links and the agent strip render. Demo rows were removed; the real DB has 0 tasks again.
- After the 2026-11-17 destructive-reset incident, the recovered UI passes `pnpm typecheck`/`build` and the same seeded visual test renders the same Today view.

## MCP (implemented 2026-11-17)

`3eccbeb` adds the loopback MCP server on `127.0.0.1:38547`:

- Agent: `http://127.0.0.1:38547/mcp`, 20 tools, bearer token in `%APPDATA%\dev.relay.app\mcp-agent.token`.
- Analyst: `http://127.0.0.1:38547/mcp/analyst`, 8 tools (job tools stubbed), separate token.
- `invoke("mcp_status")` returns `{ running, port, url, analystUrl, error, agentToken, analystToken }`; the UI still needs a Settings → Agents sheet with copy buttons and the Executor snippet.
- Agents may submit or reject a proposal; **approve is UI-only**.

## Runtime commands (already implemented)

`hover_ready`, `hover_place(x,y,w,h)`, `hover_drag`, `hover_pointer`, `hover_focus(focus)`, `hover_set_pinned(pinned)`, `hover_state`, `set_paused`, `set_dnd`, `check_for_updates`, `show_main_window` (no args), `open_main(route)`, plus `relay_snapshot`, `ack_change({taskId,changeId,rev})`, `decide_proposal({proposalId,decision,rev})`.

Events: `relay://state` (hover snapshot), `relay://open` (route string), `update://check`.

## Contract mismatches to fix (UI vs Rust)

1. **`hover_place`**: `src/ipc.ts` sends `{ mode: "tab"|"open", width, height }`; Rust expects `{ x, y, width, height }` and does not compute an edge. Pick one contract. The hover page must also snap to an edge and persist it.
2. **`relay://open` payload**: `src/ipc.ts` types it as `{ mode, edge }`; Rust emits a route string (for the tray: `/review`, `/settings`). The main window also needs to listen and navigate to a task; `App.tsx` currently only listens to `relay://change` and reads `?task=` on first load.
3. **`show_main_window`**: `src/ipc.ts` sends `{ target }`; Rust's command takes no args. Either add a `target` argument (task/focus/proposal) or switch the UI to `open_main({ route })`.
4. **Updater**: `src/updater.ts` exists (`useUpdater`, `checkForUpdates`, `downloadUpdate`, `installUpdate`, `startUpdateChecks`) but nothing calls `startUpdateChecks()` and the hover's update row is static. Wire it in `App.tsx`, and render `update://check` state.
5. **Hover entry**: add `hover.html` + `src/hover/main.tsx` + `src/hover/HoverApp.tsx` + `src/hover/hover.css` and the Vite multi-entry. The Rust window already points at `hover.html`.
6. **Review actions**: the hover must be **Review only** (opens main on the proposal). Approve/Reject stay in the main window.
7. **Counts**: hover and main use the same Today scope (`todayScope` in `src/model.ts`).
8. **Grouped rows**: no status pill inside a group; the group header carries it.

## UI status

Built (partial, uncommitted): `TitleBar`, `Rail`, `TodayHeader`, `TaskGroups`, `TaskRow`, `TaskSheet`, `ProposalCard`, `AgentStrip`, `bits`, `Icon`, mock data, `model.ts` helpers, `ipc.ts`, `types.ts`, app styles, layout tokens.

Missing/needs work: hover entry and hover React; updater wiring; empty/loading states for all views; Inbox/Projects/Agents/Review screens (the rail switches views but only Today exists); task sheet open/close via keyboard; review queue screen; real sync/update data; `pnpm build` output must include `hover.html`.

## Next Opus packages

### A. Finish the UI + hover + updater

- Implement the missing hover entry and the screens the rail exposes; reconcile the four contract mismatches above.
- Verify at 100% and 150% scaling: hover expand/collapse without focus, transparent resize flash, drag/snap across monitors, buttons on first click.
- Acceptance: `pnpm typecheck`, `pnpm build` (with `hover.html` emitted), `cargo check`; a seeded run shows Today, the task sheet, the review card and the agent strip; the hover opens on review and never approves.

### B. MCP boundary

- Read PLAN §3.3, §3.4, §4. Implement two loopback endpoints: `/mcp` (agent) and `/mcp/analyst` (reviewer), separate bearer tokens, server-enforced tool allowlists. The analyst is job-scoped, append-only, idempotent (`job_ops`), never through Executor. Kill OpenCode with a Win32 Job Object; one job at a time.
- Agent tools: `projects_list`, `tasks_list`, `tasks_get`, `tasks_search`, `tasks_changes`, `activity_list`, `tasks_create`, `tasks_update`, `tasks_claim`, `tasks_progress`, `tasks_link`, `tasks_add_requirement`, `tasks_ack_change`, `tasks_complete`, `tasks_release`, `proposals_*`, `jobs_next/get`, `sync_status/now`.
- The store already provides claims, per-claim acks, revs, changes and completion-to-review; the adapter should stay thin.
- Acceptance: initialize/tools/list/tools/call over HTTP with both tokens; analyst is refused every agent-only tool; a task claimed by two clients yields one winner; `tasks_progress` returns `unacked_changes`; `tasks_complete` lands in `review`; a timed-out analyst retry creates no duplicate task.

## Rules

- Windows first; no native dialogs except the folder picker; secrets never reach the frontend.
- DeepSeek work is `[deepseek]`-prefixed and ledgered in `docs/reviews/`; Opus writes the verdict.
- Do not push. Do not print the signing key or tokens.
- **Never run destructive git commands** (`reset --hard`, `checkout -- .`, `clean -fd`, `commit --amend`, `rebase`) while the tree has uncommitted work. The UI is uncommitted on purpose. Stage explicit paths and make a new commit.
- The real DB is clean (0 tasks). The demo seed scripts are in `%TEMP%\opencode\` and are not part of the repo.
