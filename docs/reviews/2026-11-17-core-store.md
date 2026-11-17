# 2026-11-17 — Core store, claims, changes, proposals (placeholder)

Status: placeholder; not ready for review.

This file was opened while the core store/claims package was in progress. The
core thread has since completed and the latest local commit touching
`crates/relay-core` is `c4721f9`, but the package's commands, results and risks
have not been filled in here, and the commit may still be amended. Opus should
review the final commit, not this placeholder.

## Record

- **Agent / model:** DeepSeek V4.1 Flash (`opencode-go/deepseek-v4.1-flash`),
  implementation sub-agent in the "Core store + claims" thread.
- **Commit:** `TBD`. Latest local commit touching these files at the time of
  writing: `c4721f9` — "[deepseek] Add core store, claims, changes and
  proposals" (2026-11-17). Not reviewed by this package.
- **Files:** `crates/relay-core/**`; specifically `src/model.rs`,
  `src/store/mod.rs`, `migrations/0002_change_acks.sql` (and `Cargo.toml` only
  if dependencies changed). No Tauri, no `src/`, no UI.
- **What changed (scope):** the store services behind the P1 surface —
  projects with the fixed default workflow, tasks with optimistic concurrency,
  append-only changes with per-claim acks, claims with a 30-minute lease,
  progress/heartbeat, completion to `review`, links, events, proposals,
  settings and sync cursors; `0002_change_acks.sql` adds `change_acks` and the
  claim lease index.

## Commands + results

TBD. The core agent's report should be pasted here when the package is ready,
with at least:

- `cargo test -p relay-core` (full output summary and test names)
- `cargo check`
- the migration test from `0001` to `0002` on a file DB and on an existing
  database
- the concurrent-claim and stale-rev tests, with their exact names

## Known risks

- Not reviewed yet; no pass/fail claims are made in this placeholder.
- The 30-minute lease, expiry/reclaim and clock handling need a race test, not
  just a single-threaded test.
- `0002_change_acks.sql` keeps `task_changes.acked_at` (first ack) and adds
  per-claim rows; the timeline and `unacked_changes` must agree.
- A future partial unique index for one active claim per task may be needed if
  the code-level compare-and-set is the only enforcement.
- A future unique/partial index for one live analysis job per
  conversation/watermark may be needed; the scaffold schema does not enforce
  it.

## Planned review checklist

### Claim races

- [ ] Two concurrent `claim_task` calls: exactly one wins; the loser gets a
      `ClaimConflict` carrying the current holder and claim id.
- [ ] `BEGIN IMMEDIATE` (or an equivalent write lock) is actually used for the
      compare-and-set, not a read-then-write in autocommit.
- [ ] An expired lease can be reclaimed; a released claim cannot be reused.
- [ ] A stale claim cannot write after release/expiry/reclaim.
- [ ] Every claimant write (`progress`, `link`, `ack`, `get`/heartbeat) extends
      the lease, and the extension uses one clock source.
- [ ] The active-claim lookup uses the intended index and does not scan all
      claims.

### Ack isolation

- [ ] Acks are keyed to `claim_id`: a new claimant does not inherit the
      previous claimant's acks.
- [ ] `change_acks` is unique per `(change_id, claim_id)`; the human/direct-UI
      `NULL` claim ack is idempotent (the `IFNULL` index).
- [ ] `unacked_changes(claim_id)` returns only changes for that claim's task
      that have no ack for that claim; acking again is a no-op, not an error.
- [ ] Acking a change that belongs to another task or a released claim is
      rejected.
- [ ] `task_changes.acked_at` (first ack) and the per-claim rows do not
      disagree; the timeline still shows the first ack.
- [ ] Deleting a claim cascades its acks without deleting the change history.

### complete -> review

- [ ] `complete_task` moves the task to the project's `review` status, never
      `done`.
- [ ] It releases the claim with outcome `completed` and writes the summary /
      event in the same transaction.
- [ ] A stale, released or expired claim cannot complete; the task is
      unchanged.
- [ ] `tasks_update`/the store cannot set `done` directly or clear a claim.
- [ ] A project without a `review` status fails loudly instead of using a
      fallback status.

### Rev conflicts

- [ ] `update_task` compares `expected_rev`, returns `StaleRevision` with the
      actual rev, and does not partially write.
- [ ] A successful update increments `rev` exactly once.
- [ ] Two concurrent updates: one wins, the other gets a stale-rev error; no
      lost update.
- [ ] `complete_task` and status changes also bump `rev` consistently.

### Migrations and FTS

- [ ] `0001` then `0002` apply on a fresh DB and on an existing database at
      `user_version = 1`; reopening does not re-apply.
- [ ] Foreign keys and WAL survive the migration.
- [ ] FTS5 triggers still index insert/update/delete after `0002`;
      `search_tasks` still returns the right rows.
- [ ] Migration failures roll back without a partial `user_version`.

## Opus verdict

<!-- Opus: do not fill this in until the final commit and the report above exist. -->
