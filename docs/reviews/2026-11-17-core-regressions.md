# 2026-11-17 — Core store regression tests, FTS coverage and claim busy handling

Status: complete; awaiting review.

This package closes the audit gaps listed at the end of
[2026-11-17-core-store.md](2026-11-17-core-store.md). It does not change the
public claim, ack, change or proposal semantics.

## Record

- **Agent / model:** DeepSeek V4.1 Flash (`opencode-go/deepseek-v4.1-flash`),
  test sub-agent in the "Core store + claims" thread.
- **Commit(s):** TBD at write time, by construction: this file is added by the
  package commit `[deepseek] Add core regression tests and claim busy
  handling`, so it cannot contain its own hash. Find it with
  `git log -1 --format=%H -- docs/reviews/2026-11-17-core-regressions.md`; its
  parent is `2d40aab`.
- **Files:** 2 files:
  - `crates/relay-core/src/store/mod.rs` (store fix plus tests)
  - `docs/reviews/2026-11-17-core-regressions.md` (this record)

  No `Cargo.toml`/`Cargo.lock` change (no new dependency). `src/`,
  `src-tauri/`, `design/`, `scripts/` and the rest of `docs/` are untouched.
  `docs/reviews/README.md` is not edited.

## What changed

### Store fixes (smallest change, no new product behavior)

1. **`search_tasks` passed raw user text to FTS5 `MATCH`.** The test showed
   `deploy*` prefix-matching both `Deploy relay` and `Deployment rollback`
   (2 rows) before the fix, and FTS syntax (`OR`, `NOT`, `NEAR`, column
   filters, unbalanced quotes) reaching the parser. `search_tasks` now quotes
   every whitespace-separated token, doubling embedded quotes (`fts_query`,
   `store/mod.rs`). Terms still combine with the FTS default AND, matching
   the old plain-text semantics; empty or whitespace-only input and
   `limit == 0` return an empty list instead of a `MATCH` error. No signature
   or caller change.
2. **A second connection could surface `SQLITE_BUSY` as a raw `StoreError::Sql`.**
   `Store::open` now sets a 2-second `busy_timeout` (`BUSY_TIMEOUT`) before
   WAL and migrations. A claim loser blocks on `BEGIN IMMEDIATE`, then reads
   the winner's claim inside the transaction and returns the typed
   `ClaimConflict`. Single-connection behavior is unchanged; in-memory stores
   keep using one connection.

### Tests added

- `store::tests::fts_index_tracks_tasks` — insert, update (old title/notes
  terms leave the index), archive/unarchive parity with `list_tasks`, and
  delete through the FTS trigger.
- `store::tests::fts_search_escapes_operators_and_wildcards_and_honors_the_limit`
  — literal `*`, unbalanced and balanced quotes, `OR`, `title:`, `%`, and
  exact/zero limits.
- `store::tests::upgrade_from_0001_keeps_rows_and_allows_acks` — builds a
  temp file DB with only `0001`, sets `user_version = 1`, inserts a project,
  status, task, active claim and change, then `Store::open`. Asserts
  `user_version = 2`, `change_acks` exists, all rows survive, ack works, and
  reopening skips `0002`.
- `store::tests::two_connections_race_for_the_same_claim` — two file
  connections, a `Barrier`, and two threads. Exactly one claim succeeds; the
  loser gets `ClaimConflict` with the winner's claim id and holder name; one
  claim row exists.
- `store::tests::concurrent_updates_with_the_same_expected_rev_advance_once`
  — two barrier-synchronized threads update with `expected_rev = 1`. One wins,
  the other gets `StaleRevision { expected: 1, actual: 2 }`, the stored task
  is at `rev = 2`, and exactly one `updated` event was written.
- `store::tests::human_ack_does_not_clear_an_agent_queue_and_stamps_acked_at`
  — documents the intentional first-ack semantics: a human ack
  (`claim_id = NULL`) stamps `task_changes.acked_at` and clears the human
  queue, but the agent's queue still lists the change with
  `acked_elsewhere = true`; the claim's own ack drains it, and the first-ack
  timestamp does not move.
- `TempDatabase` test helper: unique file under `std::env::temp_dir`, removes
  the DB plus `-wal`/`-shm` on drop.

## Commands + results

| Command | Result |
| --- | --- |
| `cargo test -p relay-core` | **Pass** — `45 passed; 0 failed; 0 ignored`, finished in 1.01s; doc-tests 0. Baseline before this package was 39 tests. |
| `cargo clippy -p relay-core --all-targets -- -D warnings` | **Pass** — exit 0, `Finished dev profile`, no warnings. |
| `cargo fmt --all -- --check` | **Pass** — exit 0, no diff. |
| `cargo fmt -p relay-core -- --check` | **Pass** — exit 0. |
| `cargo test -p relay-core store::tests::two_connections_race_for_the_same_claim` with `BUSY_TIMEOUT` temporarily `0ms` | **Fails 10/10** — negative control proving the test really contends on the SQLite write lock and would catch a raw lock error. The temporary edit was reverted before the package commit. |
| Same race test, 25 repetitions with the committed 2s timeout | **Pass 25/25** — 0 failures. |
| `cargo test -p relay-core store::tests::concurrent_updates_with_the_same_expected_rev_advance_once`, 25 repetitions | **Pass 25/25** — 0 failures. |

The 45 tests at this commit are the previous 39 plus the 6 new store tests
above.

## Known risks

- **SQLite busy behavior.** The 2-second busy timeout covers the claim and
  rev compare-and-set transactions, which are short. A writer that holds the
  lock longer (another long transaction, a slow disk, antivirus or a network
  filesystem) still surfaces `StoreError::Sql` with `database is locked`.
  There is no retry/backoff loop in the store; the loser's typed conflict
  depends on the winner committing inside the busy window.
- **Test flakiness.** The race tests are barrier-synchronized, not
  sleep-based, and were stable 25/25 each. They still depend on the OS
  scheduling both threads while the other holds the lock; the 0 ms negative
  control shows the contention is real, not simulated. Both concurrency tests
  need the WAL file database opened by `Store::open`; they are not meaningful
  on a single in-memory connection.
- **Clock injection is still absent.** `model::now()` and
  `model::now_plus_minutes()` read `OffsetDateTime::now_utc()` directly.
  The migration test uses a real lease 10 minutes in the future; expiry is
  still tested by backdating with direct SQL. Clock skew, DST-like cases and
  a frozen clock remain unverified.
- **`search_tasks` is literal-token AND.** A multi-word quoted phrase in user
  input is treated as separate terms, not an FTS phrase; wildcards are not
  prefix operators. This matches the fix's intent (user input cannot change
  the query), but no caller exists yet to exercise JEV's candidate shortlist.
  `search_tasks` also always excludes archived tasks; there is no
  `include_archived` parameter, unlike `list_tasks`.
- **The FTS delete test uses direct SQL.** There is no delete-task API; the
  test only proves the `tasks_fts_delete` trigger and would not catch a future
  store-level delete path that bypasses the trigger.
- **Temp files on hard failure.** `TempDatabase` cleans up on drop, but a
  process kill or a panic outside the guard's scope can leave
  `relay-test-*.db` files under the temp directory.
- **Existing core risks are unchanged and out of scope:** no partial unique
  index for one active claim, no upgrade failure injection, proposal "new
  task" apply is still unimplemented, `update_task` can still set a `done`
  status, and the store is still blocking/`Sync`. See
  `2026-11-17-core-store.md`.
- **No performance evidence.** The new tests cover correctness; no
  `EXPLAIN QUERY PLAN` was run for the FTS join or the active-claim index.

## Review checklist

Each item is `Pass` only with the cited evidence; otherwise `Unverified` or
`Fail`.

### FTS coverage

- [ ] Insert indexes title and notes; search is case-insensitive. — **Pass**,
      `fts_index_tracks_tasks` asserts `deploy`, `DEPLOY` and notes term
      `rollback`.
- [ ] Updating title/notes removes the old terms and finds the new ones. —
      **Pass**, same test: `deploy`/`rollback` empty, `canary`/`smoke` found.
- [ ] Archive hides a task from search and from the default `list_tasks`
      filter; unarchive restores it. — **Pass**, same test, and
      `include_archived` still lists the row.
- [ ] Delete removes the FTS row. — **Pass**, same test, direct
      `DELETE FROM tasks`.
- [ ] Wildcards, quotes, `OR`, column filters and `%` cannot change the
      query. — **Pass**,
      `fts_search_escapes_operators_and_wildcards_and_honors_the_limit`; the
      pre-fix `deploy*` behavior (2 rows) was observed and is recorded above.
- [ ] The limit is applied exactly, capped by candidates, and `0` is empty.
      — **Pass**, same test (`release`: 2, 10, 0).
- [ ] The fix is in the store and adds no new public API. — **Pass**,
      `fts_query` is a private helper; `search_tasks` signature is unchanged.

### Migration 0001 -> 0002

- [ ] A DB created with only `0001` and `user_version = 1` upgrades on
      `Store::open`. — **Pass**,
      `upgrade_from_0001_keeps_rows_and_allows_acks`.
- [ ] Resulting `user_version` is 2 and `change_acks` exists. — **Pass**,
      same test.
- [ ] Existing project/status/task/claim/change rows survive. — **Pass**,
      same test reads the task, claim and change after the upgrade.
- [ ] `ack_change` works after the upgrade and stamps `acked_at`. — **Pass**,
      same test.
- [ ] Reopening does not re-apply `0002`. — **Pass**, same test reopens and
      still sees version 2 and one ack row.

### Concurrency

- [ ] Two real connections on one file DB: exactly one claim wins. — **Pass**,
      `two_connections_race_for_the_same_claim`; one winner, one claim row.
- [ ] The loser gets `ClaimConflict` with the winner's holder, not a raw
      lock error. — **Pass**, same test asserts the conflict holder id and
      `holder_name`; the 0 ms negative control failed 10/10, showing the test
      exercises the lock.
- [ ] Single-connection claim behavior is unchanged. — **Pass**,
      `second_claim_is_rejected_with_the_holder` and
      `expired_lease_can_be_reclaimed` still pass.
- [ ] Two concurrent updates with the same `expected_rev`: one wins, the
      other is stale. — **Pass**,
      `concurrent_updates_with_the_same_expected_rev_advance_once`.
- [ ] The loser sees the committed rev, and the task advances by exactly 1.
      — **Pass**, same test asserts `expected: 1, actual: 2`, `rev == 2`, and
      one `updated` event.

### Ack scope

- [ ] A human ack sets `task_changes.acked_at` but leaves the agent's unacked
      queue intact. — **Pass**,
      `human_ack_does_not_clear_an_agent_queue_and_stamps_acked_at`; the
      queue entry has `acked_elsewhere = true`.
- [ ] The claim's own ack drains its queue and does not move the first-ack
      time. — **Pass**, same test.
- [ ] This is the intended semantics, not an accident. — **Pass by
      documentation**: `store/mod.rs` `ack_change` doc comment and
      `0002_change_acks.sql` both state the human `NULL` scope is separate.

### Hygiene

- [ ] No new dependency; no unrelated path touched. — **Pass**,
      `Cargo.toml`/`Cargo.lock` unchanged and the diff is the two files above.
- [ ] Temp databases are unique and cleaned up. — **Pass by inspection**,
      `TempDatabase` uses a ULID suffix and removes `-wal`/`-shm` on drop.
- [ ] `cargo test`, `cargo clippy -D warnings` and `cargo fmt --all --check`
      pass. — **Pass**, table above.

## Opus verdict

<!-- Opus: fill this in after reviewing the package commit. -->
