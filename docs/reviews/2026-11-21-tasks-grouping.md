# Grouped Tasks, Task sheet and needs-you badge (#27)

**Status:** final (local implementation, no push or PR)
**Agent / model:** OpenCode Go Hy4 Flash Free (no subagents, by thread instruction)
**Base commit:** `42e6fbb051ca99f6de8212c676671d5e16482030` (#26)
**Commit(s):** `TBD` (ledger SHA update follows)
**Files:** `crates/relay-core/migrations/v2/0002_task_progress.sql`, `crates/relay-core/src/v2/{mod,model,store,tests}.rs`, `src/{ipc,workspace,tasks.test}.ts*`, `src/App.tsx`, `src/app/{Tasks,TaskSheet,Rail}.tsx`, `src/hover/{HoverApp.tsx,hover.css}`, `src/styles/workspace.css`, this ledger

## What changed

The v2 workspace now has one pure needs-you predicate (`needsAttention`) and one attention projection (`waitingTasks` / `needsYouCount`) in `src/workspace.ts`. The rail and the Dock import those same functions, so a Notebook filter can never change the badge. Attention comes from the unfiltered snapshot: the five Needs-you statuses plus a Grilling Task whose stored turn is `user_question` or `closing_approval`. Each Task id is counted once. `groupTasks` filters with #26's shared search, then sorts Needs you, In flight and Closed by `updatedAt` descending with a ULID tie-breaker. Closed is a collapsed `<details>` group. A Grilling Task stays in In flight; `needs_answer` remains the T3-question status.

`src/app/TaskSheet.tsx` is now the read-only v2 sheet. It shows the frozen Task line, mode, status and Grilling turn, project, source line, References, Brief, pending-action summary, failure stage and reason, and the thread link, each only when present in stored data. The v1 stepper, claim banner and change-acknowledgement controls are gone. App resolves the routed id from the unfiltered workspace on every committed change, so an open sheet refreshes by id and is never hidden by a filter.

The Dock reads `v2_workspace_snapshot()` on mount, on `relay://v2-change` and on its existing timer. Its tab badge and hero chip open an accessible, keyboard-activatable list of every waiting Task, newest change first; each row calls `openMain(taskRoute(id))` and the list scrolls inside the existing Dock bounds. The v1 proposal and unacked-change count no longer drives the badge. The updater keeps its independent state: its `↑` badge and banner appear only when no Task needs the user.

The stored contract those views read lives in relay-core. `0002_task_progress.sql` adds nullable `grill_turn`, `pending_action`, `failure_stage`, `failure_reason` and `thread_url` columns to `tasks`, and the store runs v2 migrations sequentially by `user_version` (fresh stores reach version 2; a version-1 file upgrades without touching settings). `V2Task` carries these as camelCase IPC fields; a new Grill Task starts on the `agent` turn. `V2Store::update_task_progress` is the single write boundary, bumps `updated_at`, and is the unit-test fixture harness used here. No debug IPC command or frontend adapter was added: the requester asked for the existing core boundary instead, and the Tauri sheet stays read-only.

## Commands + results

| Command | Result |
| --- | --- |
| base SHA check | `42e6fbb051ca99f6de8212c676671d5e16482030` on `relay-v2/27-tasks`; clean tree at start |
| `node --version` | `v24.4.1` (portable pin prepended to `$env:Path` in the same shell as pnpm) |
| `pnpm --version` | `10.18.0` |
| `pnpm install --frozen-lockfile` | Pass, 71 packages reused, lockfile unchanged |
| `cargo test -p relay-core` | Pass, 148 passed / 0 failed / 2 ignored; doc-tests 0 (146 before this package) |
| `pnpm typecheck` | Pass, exit 0 |
| `pnpm test` | Pass, 5 files / 28 tests (4 files / 19 tests before this package) |
| `pnpm build` | Pass, 291 modules, both `index.html` and `hover.html` built |
| `cargo check --workspace` | Pass |
| `cargo fmt --all -- --check` | Pass after formatting the new Rust blocks |
| `git diff 42e6fbb051ca99f6de8212c676671d5e16482030 --check` | Exit 0 |
| `pnpm tauri dev` | Deferred: no disposable interactive WebView2 profile or live UI session in this thread |

One intermediate `pnpm typecheck` failed with `TS6133: 'isUnacked' is declared but its value is never read` after the v1 needs section was removed; the import was dropped and the re-run passed. One intermediate `pnpm test` failed because a filter fixture hid both waiting rows; the fixture was corrected to keep one, and the re-run passed.

## Stored-state regression evidence

`crates/relay-core/src/v2/tests.rs` adds two throwaway-directory tests (no live profile).

- `stored_progress_round_trips_every_status_and_the_grill_turn_flip`: a test-only SQL status writer seeds one Task per stored status, all ten statuses deserialize through a file-backed `workspace_snapshot`, a Grill Task starts on the `agent` turn, `update_task_progress` sets `user_question` plus pending action and later flips it back to `agent` without changing `status`, failure stage/reason and thread URL round-trip, and the IPC JSON keeps `grillTurn`, `threadUrl` and the snake_case `status` value.
- `a_version_one_v2_file_gains_the_progress_columns`: a real `0001_init.sql` file at `user_version = 1` opens, migrates to version 2, gains all five columns and keeps its settings row.

`src/tasks.test.ts` covers every status-to-group mapping, equal-timestamp ID tie-breaks, agent versus user Grilling turns, closing approval, `needs answer` as a T3 question, one count per Task id, newest-first waiting order, and a filter that hides waiting Tasks without changing the count or Dock list.

## Exported contracts for #35 / #36 / #37

- Grilling turn is a stored `V2GrillTurn = "agent" | "user_question" | "closing_approval"`, mirrored by `relay_core::v2::GrillTurn`. A Grill Task is created on `agent`; #35 writes `user_question` when the agent asks and `closing_approval` when the editable Brief waits for the single closing Go. Both non-agent turns need the user, while the Task stays in In flight.
- `relay_core::v2::TaskProgressUpdate` is the write contract: `grill_turn`, `pending_action`, `failure_stage`, `failure_reason`, `thread_url`, all optional (`None` clears). Write it through `V2Store::update_task_progress(task_id, update)`, which bumps `updated_at` and is transactional. #36/#37 own those producers; v2 views only read.
- The IPC `V2Task` carries `grillTurn`, `pendingAction`, `failureStage`, `failureReason` and `threadUrl` as nullable camelCase fields. The sheet renders each only when present; live editing and actions remain downstream.
- `src/workspace.ts` exports `needsAttention`, `waitingTasks`, `needsYouCount`, `NEEDS_YOU_STATUSES`, `taskGroup`, `groupTasks`, `byLatestChange` and the `TaskGroupId` / `TaskGroups` types. Both surfaces import the same predicate; keep it that way.
- Sheet mount point: `src/app/TaskSheet.tsx` exports `TaskSheet({ task, project, note, onClose })`. `src/app/Tasks.tsx` exports `Tasks` and `MissingTask`; `TaskDetail` was removed. Rail badge is still the `needsYou` prop in `src/app/Rail.tsx`.

## Known risks

- **Live desktop smoke deferred.** No disposable Windows profile or interactive WebView2 session was available, so the one-Task-per-status fixture, group order, collapsed Closed, badge of six on both surfaces, filter hiding waiting Tasks, Dock row opening the sheet across a main-window reload, and the grill-turn flip to five were not clicked through. The stored-state path, selectors, IPC shapes and rendering inputs are covered by automated Windows tests; `cargo check --workspace` and `pnpm build` cover the new wiring without launching the UI.
- **No live provider, thread or hardware case.** Slack, GitHub, ClickUp, OpenCode, T3 Code and a real thread link were not contacted; the thread is a read-only stored URL here. No Dock focus, DPI or placement hardware case was exercised; #39 owns native Dock focus and resize.
- **Grilling producer deferred to #35.** The turn fields are stored and read but nothing in this slice writes them from a live Grilling exchange; the regression uses the core fixture harness. The sheet's thread and failure fields stay empty until #36/#37 write them.
- **Workspace aggregation is still a fresh read.** Grouping and attention are pure in-memory projections of the flat snapshot; a very large Notebook re-reads every Note on each refresh. Acceptable for the local single-user v2 store.
- **BLOCKED: independent review.** The thread forbids subagents, so no second model reviewed the diff. The author reviewed it directly; the empty Opus verdict below is still required.

## Review checklist

- [x] Three groups cover all ten stored statuses; Closed starts collapsed; each group sorts by latest change with an id tie-break.
- [x] One attention predicate powers the rail badge and the Dock; filters cannot change the count or waiting list.
- [x] An agent-turn Grill is out of attention but stays In flight; user-question and closing-approval Grills need the user.
- [x] The v2 sheet is read-only and shows persisted Task, mode, status, project, References, Brief, pending action, failure and thread fields when present; v1 stepper, claims and acks are gone.
- [x] The Dock badge opens an accessible, keyboard-activatable, bounded list whose rows open `taskRoute(id)`; the updater state is untouched.
- [x] Progress fields are persisted through SQLite with a sequential v2 migration and a version-1 upgrade test.
- [x] Tracker review flags on #35, #40 and #41 were not changed; no push, PR, tracker update, tag or release was made.
- [x] `git diff --check`, fmt, typecheck, tests, build and workspace check pass on pinned Node 24.4.1.
- [ ] Opus verdict.

## Opus verdict
