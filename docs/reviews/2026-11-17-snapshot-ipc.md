# Snapshot + IPC adapter (main window)

**Status:** complete, awaiting review.
**Agent / model:** DeepSeek implementation sub-agent (DeepSeek V4.1 Flash via T3 Code / OpenCode).
**Commits:** `91e4f0a985b7d368eebdbdc03162608ca27fa09d` — "[deepseek] Add UI
snapshot and IPC commands" (2026-11-17 20:00:36 -03:00). Parent: `c4721f9`
(core store). Followed by `674c2b9`, a mechanical `cargo fmt` of
`src-tauri/src/hover.rs`.
**Files:**

- `crates/relay-core/src/view.rs` (new): the UI view model and every mapping rule.
- `crates/relay-core/src/store/mod.rs`: `Store::ui_snapshot` (one read) and `Store::get_proposal`.
- `crates/relay-core/src/lib.rs`: exports `view`.
- `src-tauri/src/commands.rs` (new): `relay_snapshot`, `ack_change`, `decide_proposal`, `CommandError`.
- `src-tauri/src/lib.rs`: registers the three commands and seeds `Inbox` on an empty database.
- `docs/reviews/2026-11-17-snapshot-ipc.md`: this record.

It does **not** touch `src/`, `design/`, the hover window or the window commands.
`docs/reviews/README.md` already existed (another package) and was not edited by
`91e4f0a`; this ledger's index is updated by the review-record commit.

## What changed

- `view.rs` mirrors `src/types.ts` field for field, with camelCase serde names. The TypeScript `Change` union is an internally tagged enum on `kind` (`requirement` / `field`); `UpdateState` is tagged on `state`.
- `Store::ui_snapshot(version, sync)` reads projects, statuses, tasks, active claims, the newest progress event per task, active changes, acks and links under one connection, then hands the rows to `view::build_snapshot`. It does not write.
- `relay_snapshot` returns that snapshot with `app.package_info().version` and an empty sync list.
- `ack_change` checks the task rev, then acks in the human scope (`claim_id = None`).
- `decide_proposal` maps `approve` / `reject`, checks the task rev when the proposal has a task, then decides as `you`.
- `CommandError { kind, message }` is camelCase, uses the store's stable `kind()` codes, and keeps the store message (so a claim conflict reads `task … is claimed by <name> (claim <id>) until <lease>`).
- On startup, an empty database gets `Inbox` (`inbox`) with the default workflow. An existing database is untouched.

## Mapping rules (as implemented)

- **Project.colorVar** — the stored color when it starts with `--` (trimmed), else `--i1..--i5` by project index (palette wraps).
- **Statuses** — canonicalized by trimmed status *name* across projects. The first project in `list_projects` order (name, case-insensitive) supplies the canonical id, category, position and display name for every name it has. Names it does not have get `status:<slug>` (lowercase, non-alphanumerics collapsed to `-`). Every project's raw status id maps to the canonical id, including the one stored on tasks. Columns sort by `todo, active, review, merged, waiting, deploying, blocked, done`, then position, then name. `backlog -> todo`; `deploying` + name containing "waiting" -> `waiting`; other `deploying` -> `deploying`.
- **Short labels** — `In Progress -> Doing`, `In Review -> Review`, `Waiting for deploy -> Deploy`, `Deployed to RC -> RC`; all other names keep their name ("Backlog", "Todo", "Merged", "Done", "Blocked") and a custom name falls back to its first word.
- **Task.ref** — `RLY-` + the last 6 characters of the ULID, uppercased.
- **Task.due** — `null` when `due_at` is null; otherwise RFC 3339 `at` plus `hasTime` = time component is not exactly `00:00:00`. An unparseable value keeps `hasTime: true` (the store validates on write).
- **Task.description** — `tasks.notes`.
- **Claim** — only an active claim (unreleased, `lease_expires_at > now`). `holder` is OpenCode when `holder_name` + `holder_kind` contains "opencode", Claude Code when it contains "claude", else T3. `lastProgressAt` is the newest `progress` event's `created_at`, else `claimedAt`; `lastProgress` is that event's body, else `"claimed"`. `leaseMinutes` is always 30; `releasedAt` is null for an active claim.
- **Changes** — active changes only, newest first (the store returns them oldest first, the adapter reverses). A `requirement` becomes `{ kind: "requirement", source, author, text, confidence, permalink, ackedAt, ackedBy }`: `source` from `source_kind` (`slack` / `github` / `clickup` / `agent`, default `slack`), `author` = `source_id` or `source_kind`, `confidence` = stored value or `0.0`, `permalink` = stored or empty. `ackedBy` is the earliest ack's `claim_id`, or `"you"` for a human (null) ack. Every other kind becomes `{ kind: "field", field: kind, from: "", to: body, ref: permalink or source_id or "" }`.
- **Links** — `t3` / `thread` / `url` / unknown -> `thread`, `pr` -> `pr`, `branch` / `commit` -> `branch`, `slack` -> `slack`, `clickup` -> `clickup`, `github` -> `github`. `label` = title or stored kind, `state` = state or `""`.
- **Proposals** — `vec![]` in P1 (JEV is P3); nothing is invented. `inboxCount` = active requirement changes with no ack at all (human scope) on visible tasks + pending proposals. `lastProposalHandledAt` = `MAX(proposals.decided_at)` or null.
- **Sync / update** — sync passes through (`[]` today). Update is `{ state: "idle", version, checkedAt: now }`.

## Commands and results

| Command | Result |
| --- | --- |
| `cargo test -p relay-core` at `91e4f0a` | Pass — 23 tests, 0 failed, 0 ignored, finished in 0.08s (4 new view tests). Re-run from a `git archive` export of the commit after `cargo clean -p relay-core`, so no artifact from another tree could be reused. |
| `cargo clippy -p relay-core --all-targets -- -D warnings` at `91e4f0a` | Pass — exit 0, no warnings (`Finished dev profile in 3.11s`). |
| `cargo check` (Tauri workspace) at HEAD | Pass — exit 0, `Finished dev profile in 1.34s`. HEAD at check time (`674c2b9`) was `91e4f0a` plus the mechanical hover.rs format; this package's code is unchanged between the two. The first run during the package warned about an unused `Manager` import in `commands.rs`; it was removed and the re-run was clean. |
| `pnpm typecheck` at HEAD | Pass — `tsc --noEmit`, exit 0. |
| `pnpm build` at HEAD | Pass — `tsc` + Vite 8.3.8, 37 modules transformed, `dist/assets/index-C6_6kFP3.js` 259.43 kB (gzip 81.26 kB), built in 229ms. |
| `pnpm typecheck` / `pnpm build` at `91e4f0a` alone | Not applicable — this commit does not contain `src/`. |

The two `pnpm` results are for the **current working tree**, not for `91e4f0a`.
They pass now only after a one-line fix that removes an unused import in
`src/components/bits.tsx`. That fix is **uncommitted** and belongs to the paused
UI package; it is not in `91e4f0a` and is not part of this record's commit. The
UI files are still untracked/modified in the working tree
(`?? src/app/`, `?? src/components/`, `?? src/data/`, `?? src/ipc.ts`, and
modified `src/App.tsx`, `src/main.tsx`, `src/styles/base.css`).

The new tests cover: the default workflow and canonical ids (`Waiting for deploy` -> `waiting`, short `Deploy`), `.ref` format, `due.hasTime` for a timed and a date-only due, OpenCode claim mapping with progress, requirement ack (`ackedBy: "you"`), field-change mapping, link kinds/labels, newest-first change order, `inboxCount = 1`, and empty proposals/sync. Three focused unit tests cover cross-project canonicalization (reuse + synthetic id), color fallback, and the serialized wire names (`inboxCount`, `lastProposalHandledAt`, `checkedAt`, `kind: "requirement"`, `"Claude Code"`).

## Known risks

- **Single canonical workflow.** The first project in name order is the reference. A project that sorts first and has custom status names becomes the canonical id source for those names; renaming or adding projects can change canonical ids. The seeded `Inbox` makes fresh installs stable, not every later edit.
- **Synthetic ids.** `status:<slug>` can collide for names that slug identically (`QA Review` vs `QA-Review`); the first row wins display, category and position. Two projects with the same custom name share the synthetic id.
- **Ref collisions.** `RLY-` + 6 ULID characters is 30 bits; it is a human label, not a key. Duplicate refs are possible and are not detected.
- **T3 holder fallback.** Anything whose name/kind does not contain "opencode" or "claude" displays as T3, including a human or a future agent. Matching is a lowercase substring over name + kind.
- **Empty proposals and sync.** The Review queue and sync dots stay empty in P1; `inboxCount` still adds pending proposal rows once they exist, but no proposal is surfaced as a card.
- **Snapshot window.** Tasks are capped at 1000 (same as `list_tasks`), archived tasks are excluded, and only `active` changes are exposed. A retracted/superseded change is absent from the timeline by design; a larger database silently drops tasks past the cap.
- **Confidence default.** A requirement stored without `confidence` shows `0.00`; nothing invents a score.
- **Ack attribution.** `ackedBy` is the earliest ack (claim id or "you"). A later human ack after an agent ack still displays the agent claim id.
- **Due parse fallback.** A malformed `due_at` cannot come through the store, but if a row is edited outside it, the UI sees the string with `hasTime: true`.
- **Working tree note.** `cargo fmt --all` also normalised whitespace in
  `src-tauri/src/hover.rs`, which had uncommitted edits from the hover package.
  No hover behaviour or command signature changed, and `hover.rs` is not part of
  this commit; the formatting was committed separately as `674c2b9`. At HEAD,
  `cargo fmt --all -- --check` still fails on 3 hunks in
  `src-tauri/src/lib.rs` (hover-window code, outside this package), while
  `cargo fmt -p relay-core -- --check` is clean.

## Review checklist

1. Diff `crates/relay-core/src/view.rs` structs against `src/types.ts`; every required field should be present with the same nullability.
2. Check the status canonicalization and category map against `docs/PLAN.md` section 2, especially `backlog -> todo` and `Waiting for deploy -> waiting`.
3. Check claim selection (unreleased + unexpired), holder heuristics and the progress fallback.
4. Check change ordering, `ackedBy`, and the field-change projection.
5. Check that `inboxCount` uses human-scope unacked requirements plus pending proposals.
6. Check the three command signatures and the rev guards; confirm `relay_snapshot` uses the package version.
7. Confirm the `Inbox` seed only runs when `list_projects()` is empty, and that no hover/window command changed.
8. Confirm `src/` and `design/` are untouched by the commit.

## Opus verdict

<!-- Opus: fill this in after reviewing 91e4f0a. -->

## Opus verdict (2026-11-17)

**Verdict:** approved with notes
**Reviewed:** `91e4f0a` (line numbers at `91e4f0a`)

What holds:

- The snapshot exposes only display data. It includes no tokens, no
  `metadata_json`, no raw messages and no `jev_json`. Tasks are filtered to
  `archived = 0`, and claims to unreleased claims with a lease still running
  (`crates/relay-core/src/store/mod.rs:305` onward).
- All reads run while one `Store` mutex is held (`store/mod.rs:305`), so an
  in-process MCP write cannot land in the middle of a snapshot.
- `decide_proposal` cannot bypass the rev rule. The command's rev check is
  advisory (`src-tauri/src/commands.rs:89`). The store then applies the patch at
  the proposal's `base_rev` inside its own `Immediate` transaction
  (`store/mod.rs:1540` and `:2186` at `c4721f9`), so a stale proposal fails
  atomically.
- The commands stay thin and map `StoreError::kind()` to a stable code
  (`commands.rs:47-95`). No command takes or forges a `claim_id`.

What I would change:

- `ack_change` checks the rev of `task_id` but then acks any `change_id`. It
  never checks that the change belongs to that task (`commands.rs:55-65`). Fix:
  load the change and reject it when `change.task_id != task_id`.
- `inbox_count` includes pending proposals, but `proposals` is always empty
  (`crates/relay-core/src/view.rs:469`, `:475`). Once agents can call
  `proposals_submit`, the badge will count items the UI cannot show. Fix: ship
  the proposal rows, or leave them out of the count until P3.
- The snapshot runs several `SELECT`s without a read transaction
  (`store/mod.rs:305-400`). Once a second process writes, for example the Slack
  capture, the snapshot can mix two states. Fix: wrap the reads in one deferred
  transaction.

Nothing blocking.

Not covered: the UI's actual use of `rev` and `taskRev`, and snapshot cost with
1000 tasks (the query caps at `MAX_TASK_LIMIT` without saying so).

