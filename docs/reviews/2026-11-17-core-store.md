# 2026-11-17 — Core store, claims, changes, proposals

Status: complete; awaiting review.

The core thread is closed and the package is committed. Opus should review
`c4721f9`, not an earlier state of the branch.

## Record

- **Agent / model:** DeepSeek V4.1 Flash (`opencode-go/deepseek-v4.1-flash`,
  max variant), implementation sub-agent in the "Core store + claims" thread.
- **Commit:** `c4721f91045ca4e35007cb52bcbf6f3dc09b6f16` — "[deepseek] Add core
  store, claims, changes and proposals" (2026-11-17 19:47:27 -03:00). Its parent
  is `1e26ee3` (hover window). The UI snapshot (`91e4f0a`) and the hover
  formatting commit (`674c2b9`) come after it and are separate packages.
- **Files:** 3 files, 3,432 insertions, 175 deletions:
  - `crates/relay-core/src/model.rs`
  - `crates/relay-core/src/store/mod.rs`
  - `crates/relay-core/migrations/0002_change_acks.sql`

  `crates/relay-core/src/lib.rs` is **not** in this commit. `pub mod model;` and
  `pub mod store;` already existed from the scaffold; `pub mod view;` was added
  later by `91e4f0a`. No `src/`, `src-tauri/`, `design/` or UI file is touched,
  and `Cargo.toml`/`Cargo.lock` do not change (no new dependency).

## What changed

- **Projects and the default workflow.** `create_project_with_default_workflow`
  seeds the fixed P1 workflow: Backlog, Todo, In Progress, In Review, Merged,
  Waiting for deploy, Deployed to RC, Done, Blocked (`store/mod.rs:44-56`).
  `Deployed to RC` is `deploying` and `Done` is the only `done` category.
  Project slugs are unique (`project_creation_rejects_a_duplicate_slug`).
- **Tasks and `rev`.** A task starts at `rev = 1`. `update_task` takes an
  explicit `expected_rev`, compares it inside `BEGIN IMMEDIATE`, returns
  `StaleRevision { expected, actual }`, and increments `rev` exactly once per
  successful patch (`apply_task_patch`, `store/mod.rs:2308-2439`). List filters
  cover project, category, text, `updated_since`, archived and limit.
- **Claims, lease and compare-and-set.** `claim_task` runs in
  `BEGIN IMMEDIATE` (`store/mod.rs:1003`), rejects archived tasks, returns
  `ClaimConflict` with the current holder when an unexpired claim exists, closes
  expired unreleased claims as `expired` in the same transaction, then inserts
  the new claim with a 30-minute lease (`CLAIM_LEASE_MINUTES`, `store/mod.rs:39-40`).
  `heartbeat`, `add_progress`, `ack_change`, `add_change` and `add_link` all call
  `extend_claim` to push the lease to now + 30 minutes. A released or expired
  claim fails `ClaimReleased` / `ClaimExpired` (`ensure_claim_active`,
  `store/mod.rs:2185-2193`).
- **Per-claim acks.** `0002_change_acks.sql` adds `change_acks(change_id,
  claim_id, acked_at)` with `ON DELETE CASCADE` on both parents, a unique index
  on `(change_id, IFNULL(claim_id, ''))` and an index on `claim_id`. A claimant
  ack requires a live claim that owns the change's task and extends its lease; a
  human/direct-UI ack uses `claim_id = NULL` and is a deliberately separate
  scope. `INSERT OR IGNORE` makes a repeated ack a no-op. The first ack of any
  scope stamps `task_changes.acked_at` with `COALESCE` for the timeline
  (`store/mod.rs:897-963`). `unacked_changes(claim_id)` lists active changes of
  that claim's task with no row for that claim, flagging `acked_elsewhere` when
  another scope acked (`store/mod.rs:2208-2239`).
- **Why `0002` is a table, not a column.** An ack belongs to a
  `(change, claim)` pair: a new claimant must not inherit the previous
  claimant's acks, so one `acked_at`/`acked_by` column cannot describe the
  queue. The human/direct-UI scope is a third case that a nullable column would
  have to encode with a sentinel; here it is explicit as `claim_id = NULL`, and
  the `IFNULL` index keeps it idempotent even though SQLite treats NULLs as
  distinct. `task_changes.acked_at` stays as the first-ack timeline value while
  the table holds per-scope truth.
- **Requirements, retraction and supersession.** Changes are append-only with
  `state` `active` / `retracted` / `superseded`. `retract_change` and
  `supersede_change` change state and write an event; rows are never deleted.
  Retracting a superseded change and superseding a retracted one are rejected.
  Both operations are idempotent for a repeated identical call.
- **Progress and heartbeat.** `add_progress` appends a `progress` event, extends
  the lease and returns the claim's remaining unacked changes in one
  transaction; `heartbeat` only extends the lease.
- **Complete to review.** `complete_task` moves the task to the project's
  `review` status (never `done`), stamps `completed_at`, bumps `rev`, writes a
  `completed` event carrying the summary and releases the claim as `completed`,
  all in one transaction (`store/mod.rs:1157-1204`). Replaying completion for the
  same claim returns the task; other writes on a completed claim fail.
- **Links, events, proposals, settings, cursors.** Links dedupe by external id
  or URL. Events are appended with actor name/kind and a JSON body. Proposals
  store a `TaskPatch` JSON and a `base_rev`; approving applies only known
  `TaskPatch` fields with the revision guard, ignores unknown fields, and leaves
  the proposal pending on a stale revision. Settings and sync cursors upsert by
  key/connector+scope.

## Commands + results

"At `c4721f9`" runs used a `git archive` export of that commit in a temp
directory and the repository's shared Cargo target directory, after
`cargo clean -p relay-core`, so the binary could not be a reused artifact from
another tree.

| Command | Result |
| --- | --- |
| `cargo test -p relay-core` at `c4721f9` | Pass — `19 passed; 0 failed; 0 ignored`, finished in 0.07s; doc-tests 0. |
| `cargo clippy -p relay-core --all-targets -- -D warnings` at `c4721f9` | Pass — exit 0, no warnings (`Finished dev profile in 2.56s`). |
| `cargo fmt -p relay-core -- --check` at `c4721f9` | Pass — exit 0. |
| `cargo fmt --all -- --check` at `c4721f9` | **Fail** — exit 1. Diffs are outside the package: 17 hunks in `src-tauri/src/hover.rs` and 3 hunks in `src-tauri/src/lib.rs` (hover-window code from `1e26ee3`). Not fixed by this package. |
| `cargo test -p relay-core` at `674c2b9` (current tree) | Pass — `23 passed; 0 failed; 0 ignored`, finished in 0.08s. The extra 4 are `view::tests` from `91e4f0a`. |
| `cargo clippy -p relay-core --all-targets -- -D warnings` at `674c2b9` | Pass — exit 0, no warnings. |
| `cargo check` (Tauri workspace) at `674c2b9` | Pass — exit 0, `Finished dev profile in 1.34s`. |
| `cargo fmt --all -- --check` at `674c2b9` | **Fail** — exit 1, 3 hunks in `src-tauri/src/lib.rs`. `hover.rs` is clean because `674c2b9` formatted it. `cargo fmt -p relay-core -- --check` is clean. |

The 19 tests at `c4721f9`:

```
model::tests::task_patch_distinguishes_absent_from_null
model::tests::timestamps_are_fixed_width_rfc3339
model::tests::models_are_camel_case
model::tests::status_categories_round_trip
store::tests::filters_and_substring_search
store::tests::default_workflow_matches_the_plan
store::tests::project_creation_rejects_a_duplicate_slug
store::tests::expired_lease_can_be_reclaimed
store::tests::complete_moves_to_review_and_releases_the_claim
store::tests::proposals_apply_only_known_fields
store::tests::changes_retract_and_supersede_without_deleting
store::tests::add_event_and_list_events
store::tests::file_database_is_wal_and_migrated
store::tests::progress_extends_the_lease_and_returns_unacked_changes
store::tests::links_dedupe_by_external_id_or_url
store::tests::acks_are_isolated_between_claims
store::tests::settings_and_sync_cursors_round_trip
store::tests::second_claim_is_rejected_with_the_holder
store::tests::task_round_trip_and_stale_rev
```

The 4 tests added by `91e4f0a` (23 at HEAD): `view::tests::snapshot_maps_the_store_to_the_ui_contract`,
`view::tests::statuses_are_canonical_across_projects`,
`view::tests::project_colors_use_the_palette_unless_they_are_variables`,
`view::tests::wire_json_uses_the_typescript_names`.

Not run / not available for this package:

- `pnpm typecheck` / `pnpm build` do not exercise `relay-core`; they pass in the
  current tree (`674c2b9`) and are recorded in `2026-11-17-snapshot-ipc.md`.
- No test upgrades an existing database from `user_version = 1` to `2`. The only
  migration test (`file_database_is_wal_and_migrated`) opens a fresh file DB and
  asserts WAL plus `user_version = 2`.
- The scaffold's `migrations_and_task_round_trip` and `fts_index_tracks_tasks`
  tests are not in `c4721f9` and were not replaced: nothing in the crate,
  including the tests, calls `search_tasks`, and no test exercises the FTS5
  triggers. This is a coverage regression, not evidence of a code defect.

## Known risks

- **The concurrent-claim test is not concurrent.** `second_claim_is_rejected_with_the_holder`
  claims twice sequentially on one in-memory `Store`; there are no threads and no
  second connection in the test. The `BEGIN IMMEDIATE` compare-and-set is the
  right mechanism, but a real two-connection race on a file DB is unproven.
- **The lease clock is not injectable.** `model::now()` and `model::now_plus_minutes()`
  read `OffsetDateTime::now_utc()` directly (`model.rs:28-35`). Tests backdate a
  lease with direct SQL (`expire_claim` helper), so clock changes or DST-like
  edge cases are not simulated.
- **Proposal "new task" apply is not implemented.** Approving requires a
  `task_id` and a `base_rev`; otherwise `decide_proposal` returns `InvalidInput`
  (`store/mod.rs:1648-1657`). A proposal with `task_id: None` can be created but
  never approved.
- **No partial unique index for one active claim.** Enforcement is only the
  code-level CAS inside `BEGIN IMMEDIATE`; `idx_claims_task_lease` is not unique.
  A second writer that bypasses this store (or a future migration bug) could
  create two active claims.
- **No idempotency / `job_ops` store API.** `job_ops` exists in `0001` with
  `UNIQUE (job_id, op_index)`, but no store method reads or writes it, so job
  operation idempotency is deferred.
- **The store is blocking and `Sync`.** `Store` is `{ conn: Mutex<Connection> }`
  and every method is a synchronous `&self` call. The Tauri commands are plain
  `pub fn` (`src-tauri/src/commands.rs:47,55,71`) without `spawn_blocking`; an
  async caller must move these calls off the async runtime.
- **`update_task` can set any project status, including `done`.** `apply_task_patch`
  only checks that the status belongs to the task's project
  (`store/mod.rs:2372-2384`). The "complete -> review" rule is enforced only in
  `complete_task`, not in the generic patch path.
- **No upgrade test for `0001 -> 0002`.** `migrate` skips applied versions
  (`store/mod.rs:1847-1860`), but no test opens a DB at `user_version = 1` with
  existing rows, upgrades it, and reopens. Migration failures are rolled back
  inside one `unchecked_transaction`, but no failure injection proves it.
- **FTS coverage was dropped** with the scaffold test (see above). The triggers
  exist in `0001`, but `search_tasks` has no caller in the crate and nothing
  verifies the triggers after `0002` or after task update/delete.
- **First-ack semantics.** `task_changes.acked_at` is the first ack of *any*
  scope. A human ack sets it while an agent claim's queue can still list the
  change as unacked (`acked_elsewhere = true`), and the UI's `ackedBy` shows the
  earliest ack, not the current claim's.

## Review checklist

Each item is `Pass` only with the cited evidence; otherwise `Unverified` or
`Fail`.

### Claim races

- [ ] Two concurrent `claim_task` calls: exactly one wins, the loser gets
      `ClaimConflict` with the holder and claim id. — **Unverified** for
      concurrency. The sequential case is `Pass` via
      `second_claim_is_rejected_with_the_holder` (asserts holder id and
      `holder_name`).
- [ ] `BEGIN IMMEDIATE` is used for the compare-and-set. — **Pass**,
      `store/mod.rs:1003`.
- [ ] An expired lease can be reclaimed; a released claim cannot be reused. —
      **Pass**, `expired_lease_can_be_reclaimed` (old claim closed as `expired`,
      second claim becomes active) and `active_claim_for_task` filters
      `released_at IS NULL` (`store/mod.rs:2153-2163`).
- [ ] A stale claim cannot write after release/expiry/reclaim. — **Pass**,
      `expired_lease_can_be_reclaimed` asserts `heartbeat`, `add_change` and
      `add_progress` fail after expiry/reclaim; `complete_moves_to_review_and_releases_the_claim`
      asserts a released claim cannot be re-released.
- [ ] Every claimant write extends the lease from one clock source. — **Pass by
      inspection**: `extend_claim` at `store/mod.rs:736` (`add_change`), `916`
      (`ack_change`), `1296` (`add_link`), plus `add_progress` and `heartbeat`;
      all use `now_plus_minutes`. No test asserts the new lease value for every
      write.
- [ ] The active-claim lookup uses the intended index and does not scan all
      claims. — **Unverified**; `idx_claims_task_lease` exists in `0002` but no
      `EXPLAIN QUERY PLAN` was run.

### Ack isolation

- [ ] Acks are keyed to `claim_id`; a new claimant does not inherit the
      previous claimant's. — **Pass**, `acks_are_isolated_between_claims`
      (asserts `unacked.len() == 1` for the second claim after the first acked).
- [ ] `change_acks` is unique per `(change_id, claim_id)` and the human `NULL`
      ack is idempotent. — **Pass**,
      `0002_change_acks.sql:17-18` (`IFNULL(claim_id, '')`) plus
      `acks_are_isolated_between_claims` replaying the human ack.
- [ ] `unacked_changes(claim_id)` scopes to the claim's task and a repeat ack is
      a no-op. — **Pass**, `store/mod.rs:2208-2239` and `INSERT OR IGNORE`
      (`store/mod.rs:922-926`); covered by `acks_are_isolated_between_claims`.
- [ ] Acking a change of another task or on a released claim is rejected. —
      **Pass by inspection**, `ClaimTaskMismatch` at `store/mod.rs:909-914` and
      `ensure_claim_active` at `915`; no dedicated test for the ack path.
- [ ] `task_changes.acked_at` (first ack) and the per-claim rows agree; the
      timeline shows the first ack. — **Pass by inspection**,
      `COALESCE(acked_at, ...)` at `store/mod.rs:928-931`; see the first-ack
      risk above.
- [ ] Deleting a claim cascades its acks without deleting change history. —
      **Pass by schema**, `ON DELETE CASCADE` on `change_acks` in
      `0002_change_acks.sql:9-11`; there is no claim-delete API to exercise it.

### complete -> review

- [ ] `complete_task` moves to the project's `review` status, never `done`. —
      **Pass**, `complete_moves_to_review_and_releases_the_claim` asserts the
      review status id.
- [ ] It releases the claim as `completed` and writes the summary/event in the
      same transaction. — **Pass**, test asserts `outcome = "completed"`, no
      active claim, one `completed` event with the summary; code at
      `store/mod.rs:1159-1203`.
- [ ] A stale, released or expired claim cannot complete; the task is unchanged.
      — **Pass by inspection**, `ensure_claim_active` at `store/mod.rs:1162-1170`;
      the test covers the released/replayed case. Expired-complete is not
      separately tested.
- [ ] The store cannot set `done` directly or clear a claim. — **Fail for the
      `done` half**: `update_task` accepts any status of the project, including
      a `done` status (`store/mod.rs:2372-2384`). The patch has no claim field,
      so it cannot clear a claim. Deciding whether direct-to-`done` is intended
      (a human may move a card to Done) is an Opus question.
- [ ] A project without a `review` status fails loudly. — **Pass by inspection**,
      `InvalidInput("project ... has no review status")` at
      `store/mod.rs:1173-1175`; not tested.

### Rev conflicts

- [ ] `update_task` compares `expected_rev`, returns `StaleRevision` with the
      actual rev, and does not partially write. — **Pass**,
      `task_round_trip_and_stale_rev` asserts expected 1 / actual 2 and that the
      stale `notes` did not land.
- [ ] A successful update increments `rev` exactly once. — **Pass**, same test
      asserts `rev == 2` after one patch.
- [ ] Two concurrent updates: one wins, the other gets stale-rev. —
      **Unverified**; no concurrency test. The guard runs inside
      `BEGIN IMMEDIATE` (`store/mod.rs:642`).
- [ ] `complete_task` and status changes bump `rev` consistently. — **Pass**,
      `complete_moves_to_review_and_releases_the_claim` asserts `rev == 2`;
      status patches go through `apply_task_patch`'s `rev = rev + 1`.

### Migrations and FTS

- [ ] `0001` then `0002` apply on a fresh DB. — **Pass**,
      `file_database_is_wal_and_migrated` asserts WAL and `user_version = 2`.
- [ ] `0001 -> 0002` applies on an existing database at `user_version = 1`, and
      reopening does not re-apply. — **Unverified**; no test creates a v1 DB
      with rows. The skip logic is `store/mod.rs:1851-1853`.
- [ ] Foreign keys and WAL survive the migration. — **WAL Pass** (test);
      **Unverified** for a post-migration foreign-key assertion.
- [ ] FTS5 triggers still index insert/update/delete after `0002`;
      `search_tasks` returns the right rows. — **Unverified**: `search_tasks` has
      no caller in the crate and no test; the scaffold's `fts_index_tracks_tasks`
      was dropped.
- [ ] Migration failures roll back without a partial `user_version`. — **Pass by
      inspection**, `unchecked_transaction` wraps the batch and the pragma
      (`store/mod.rs:1854-1857`); no failure-injection test.

## Opus verdict

<!-- Opus: fill this in after reviewing c4721f9. -->
