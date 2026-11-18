# Slack capture, JEV jobs and the headless reviewer

**Status:** complete; live loop verified with a simulated DM (no Slack workspace yet).
**Agent / model:** DeepSeek V4.1 (orchestrator); the Slack/JEV UI is Opus 5.5 (`e16d3f8`).
**Commits:** `df5f34a` (store + schema), `17b1d57` (runtime + analyst scoping), `8016700`
(runner), `f5ca859` (created-task scope + rescreen), `c5a4fc6` (git/GitHub clients).
**Files:** `crates/relay-core/{model,analysis,slack,git,github}.rs`, `store/mod.rs`,
`migrations/0003_slack_jobs.sql`, `src-tauri/src/{slack,analyst}.rs`, `mcp/{analyst,tests}.rs`,
`src/ipc.ts`, `src/app/Slack.tsx`, `docs/SLACK_SETUP.md`.

## What changed

### Store and schema (P2/P3 data)

- `0003_slack_jobs.sql` extends the 0001 tables instead of duplicating them:
  `slack_peers` for exclusion and names; `messages` gains `edited_at`, `deleted`,
  `historical`, `created_at`; `analysis_jobs` gains `candidates_json`, `route`, `summary`,
  `updated_at`, `superseded_by`; a unique `(conversation_id, watermark)` index.
- `conversations` is the episode table: gap segmentation (90 min), thread replies attach to
  the parent episode (within 24 h), a closed episode reopens on a new message.
- `capture_slack_message` dedupes by `(channel, ts)` and updates edits in place; an excluded
  peer writes nothing.
- Jobs: one per `(conversation, watermark)`; a newer watermark supersedes live jobs.
  `record_job_op` makes analyst writes idempotent; `add_job_candidate` puts a task the
  reviewer created in scope for a due/priority proposal.
- `clear_slack_capture` deletes captured DMs, episodes and jobs; tasks stay.

### Slack client and runtime

- `relay-core/src/slack.rs`: `auth.test`, `apps.connections.open`, paginated
  `conversations.list`, `conversations.history`, `users.info`, Socket Mode frame parsing,
  and `message_changed` / `message_deleted` handling. Tokens only in the `Authorization`
  header; `Debug` redacts them. Mock-server tests cover pagination, edits, errors and frames.
- `src-tauri/src/slack.rs`: keyring-backed tokens (`dev.relay.app`), a Socket Mode loop with
  ack-first envelopes, reconnects with backoff, catch-up on `hello` (14-day first backfill,
  marked `historical`), name cache, and JEV analysis per captured episode.
- Global "Pause capture" is honoured before any write.

### JEV analysis (two stages)

- `relay-core/src/analysis.rs` holds the stage A/B question sets, the 12k transcript
  trimming (first + newest) and `merge_answers`, which feeds the stage A screen answer into
  the stage B routing.
- `route()` now returns `Rescreen` for the middle band (the plan's "rescreen at close"),
  not `Review`. `needs_human`, `multiple`, conflicting answers and missing answers still
  review.
- A route becomes a job: ignore → done/analyzed; review → pending/needs_review; change or
  new task → `pending_agent` and the runner starts; rescreen stays open with no job;
  historical backfill → `shadow` (answers stored, no action).

### Headless reviewer

- `src-tauri/src/analyst.rs` writes an isolated OpenCode config (only the analyst MCP, a
  `relay-analyst` agent, `bash`/`edit`/`write`/`webfetch` disabled), runs
  `opencode run --agent relay-analyst --model opencode-go/deepseek-v4.1-flash --archived
  --auto --format json`, one job at a time, with a 3-minute timeout and a Win32 Job Object
  with `KILL_ON_JOB_CLOSE`. Logs go to `%APPDATA%\dev.relay.app\analyst\run-<job>.log`.
- If the process exits without `jobs_complete`, the job is marked failed with the reason.

### Analyst endpoint (real jobs)

- `jobs_get` returns the job, the episode and candidate task snapshots.
- Writes are job-scoped (`forbidden` for a non-candidate) and idempotent by
  `<jobId>:<opIndex>`; a replay returns the first result. Analyst proposals still only
  touch status/due/priority.

### UI (Opus, `e16d3f8`)

- Settings → Slack (status, token form, disconnect, peer exclusion) and JEV (key save/clear
  with one validating call); Inbox episode list with state tabs; transcript sheet with
  "Analyze now"; Review → "From Slack"; `/inbox?episode=<id>`; MCP tokens masked with
  Reveal/Copy.

## Commands and results

| Command | Result |
| --- | --- |
| `cargo test -p relay-core` | **65 passed** |
| `cargo test -p relay --lib` | **21 passed** |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `pnpm typecheck`, `pnpm vitest run`, `pnpm build` | pass (11 tests) |

## Live loop (simulated DM, real JEV and real OpenCode)

Two debug-only DMs from `U-DEMO3` ("Please create a brand new task called Verify the RC
build…", "Yes, that is the final decision…") were captured into one episode, then
`episode_analyze` ran.

- Stage A: `worth_reviewing` **0.98**, `conversation_state` `waiting_on_me`.
- Stage B: `affects_existing` 0.14, `task` `none`, `change_kind` `new_task` (1.00),
  `is_new_task` **0.89**, `conversation_complete` **0.90**, `needs_human` 0.22, `urgency`
  1.93, `project` `unsure`.
- Route `new_task` → job `pending_agent`, runner started (`attempts` 1, state `running`).
- The reviewer created **"Verify the RC build"** in Inbox/Todo with timestamps in the notes
  and finished with `jobs_complete`; the job is `done`, `attempts` 1.
- The task was archived and the capture cleared after the test; the DB is back to 0 active
  tasks and 0 episodes.

The same run showed the boundary working: the reviewer tried `proposals_submit` on the task
it had just created and was refused (`forbidden`) until `add_job_candidate` was added.

## Known risks

- **No live Slack workspace.** The socket path has never talked to Slack; only the HTTP
  client and frame parser are covered by mocks. The user token may need workspace approval.
- **Keyring** was exercised on this machine (`jev_set_key` then analysis), but not across a
  reboot.
- **OpenCode config** uses the `mcp.<name>` and `agent.<name>` forms; the current schema
  accepts them but marks them deprecated. If OpenCode removes them, switch to
  `mcp.servers` + `agents` (`system`/`permissions`).
- **One reviewer at a time.** A burst of episodes leaves later jobs `pending_agent` until
  another capture or a manual run.
- **Commit `8016700` also contains the UI agent's in-progress `src/` files** (a `git add -A`
  swept them up mid-work). The code is final: `e16d3f8` added the rest and its typecheck,
  tests and build pass on the same tree. Only the commit boundary is wrong.
- **Cost:** every episode runs two JEV calls; a `pending_agent` job runs one DeepSeek
  OpenCode session. Shadow backfill runs JEV for 14 days of DMs.
- **No job retry UI:** a failed job stays failed with its error; there is no button to
  requeue it yet.

## Review checklist

- [ ] Does `capture_slack_message` keep one episode per gap and attach thread replies?
- [ ] Is a replayed `(channel, ts)` an update, never a duplicate?
- [ ] Does an excluded peer write nothing?
- [ ] Is the watermark unique per conversation, and is a newer job superseding the older?
- [ ] Does a replayed analyst op return the first result, including a created task?
- [ ] Are analyst writes limited to the job's candidates (plus tasks it created)?
- [ ] Is the runner's process tree killed on timeout (Job Object), and is the job failed
      when `jobs_complete` never arrives?
- [ ] Does the UI mask MCP tokens (Rust still sends the full value to the frontend)?

## Opus verdict (2026-11-17)

**Verdict:** changes needed
**Reviewed:** `df5f34a`, `17b1d57`, `8016700`, `f5ca859`, read at those commits and at
`163d5f6` (HEAD). The `e16d3f8` UI and `c5a4fc6` are out of scope here. Tests were not re-run.

### What holds

- Capture runs in one `IMMEDIATE` transaction. A replayed `(channel, ts)` updates the row
  in place and never inserts a second one (`crates/relay-core/src/store/mod.rs:2076-2127`).
  An excluded peer stops before any message or episode write (`store/mod.rs:2087-2090`).
- Job watermarks are unique in the schema (`migrations/0003_slack_jobs.sql:24-25`).
  `create_job` returns the existing row for a repeated watermark and supersedes older live
  jobs in the same transaction (`store/mod.rs:2416-2441`).
- Routing compares strictly against the plan's thresholds (`jev.rs:515-600`):
  `worth <= 0.60` rescreens, `needs_human > 0.60` reviews, and these cases also review:
  both scores high, `task = multiple`, `affects_existing` without a task, and any missing
  answer.
- The analyst boundary checks the candidate list before any write
  (`src-tauri/src/mcp/analyst.rs:198-207`). The idempotency key must name the same job
  (`analyst.rs:210-219`). An analyst proposal may only touch `statusId`, `dueAt` and
  `priority` (`store/mod.rs:1617-1625`).
- The Socket Mode ack is sent before the event is handled (`src-tauri/src/slack.rs:290-299`).
- A timeout terminates the Job Object. An exit without `jobs_complete` marks the job failed
  (`src-tauri/src/analyst.rs:136-156`, `37-58`).
- Tokens are stored under keyring service `dev.relay.app`, and `SlackClient`'s `Debug`
  redacts both tokens (`crates/relay-core/src/slack.rs:80-87`).

### What I would change

Blocking:

1. **The reviewer is not isolated.** `OPENCODE_CONFIG` adds a config file; OpenCode still
   merges it with the global `~/.config/opencode/opencode.json`. On this machine that file
   loads the `executor` MCP server. The log of the "verified" live run
   (`%APPDATA%\dev.relay.app\analyst\run-01M7P3Q94TZWWTGVNZ0TTTWTPC.log`) shows the reviewer
   calling Executor's `search` and `execute`, under `--auto`. Every Relay call in that run
   went through Executor code mode. This breaks PLAN 3.3 and 6: "cannot reach Executor,
   Hopper or Spawner." See `src-tauri/src/analyst.rs:101-121` and `173-200`.
   Fix: point the global config location at an empty directory for the child (for example
   `XDG_CONFIG_HOME` / `OPENCODE_CONFIG_DIR`). Deny tools by default (`"*": false`) and
   allow only `relay-analyst_*`. Fail the job if the run log shows any other tool.
2. **The peer is taken from the message author, not from the DM.** Code:
   `src-tauri/src/slack.rs:427`, `store/mod.rs:2146-2156` and `2188`. The user's own
   replies create a "self" peer. Episodes are looked up by peer rather than channel, so the
   user's replies from every DM collect in one episode. Every message is stored as
   `direction = "in"`. Excluding a peer does not exclude the user's replies to that peer.
   The live test sent only inbound messages from one peer, so it did not hit this.
   Fix: pass the DM's counterpart (from `list_dms` or `conversations.info`) as the peer. Key
   episode reuse by `channel_id`. Derive the direction by comparing the author with that
   peer.
3. **Analyst writes ignore the job state, and idempotency is not atomic.** Code:
   `src-tauri/src/mcp/analyst.rs:232-289` and `313-317`.
   - Writes and `jobs_complete` are accepted on `done`, `failed`, `superseded` and `shadow`
     jobs. `jobs_complete` overwrites `superseded` with `done`.
   - The task, change or proposal is written in one transaction and `job_ops` in another. A
     kill between the two, or two concurrent calls with the same key, creates a duplicate.
   - Key `<job>:-1` passes `op_slot`, the write happens, and only then does
     `record_job_op` reject the negative index.
   - A replay returns the first result even when the tool differs.

   Fix: do the write and the `job_ops` insert in one store transaction. Require
   `state = 'running'`. Reject `index < 0` in `op_slot`. Check that `op.tool` matches.
4. **Re-analysing an existing watermark resets the job and starts the reviewer again.**
   Code: `src-tauri/src/slack.rs:589-619`. An edit, a redelivered envelope, or a catch-up
   that re-captures the newest message keeps the same watermark. `create_job` returns the
   existing job, and `set_job_status` then moves a `done` or `failed` job back to
   `pending_agent` and calls `spawn_job`.
   Fix: have `create_job` report whether it inserted a row, and skip the status change and
   the spawn when it did not.
5. **Retention is never run.** `prune_slack_messages` (`store/mod.rs:2366`) has no caller.
   It also prunes by capture time, so a 14-day backfill message survives 44 days. The
   reviewer logs keep DM text indefinitely (`analyst.rs:93`), and `clear_slack_capture`
   does not remove them.
   Fix: prune daily on the message `ts` with a 30-day default. Delete `analyst/run-*.log`
   in both prune and clear.
6. **Catch-up ignores Pause.** `catch_up` (`src-tauri/src/slack.rs:347-405`) captures and
   runs JEV on every `hello` while capture is paused. Only `handle_event` checks the pause
   (`slack.rs:316`).
   Fix: check `paused(&app)` before listing DMs and before each capture and analysis.

Should fix before 1.0:

- **Deletes are dropped.** `message_deleted` has `user_id: None`, so `capture` returns at
  `slack.rs:427`. Deleted DMs stay stored and are still sent to JEV.
  Fix: mark the message deleted by `(channel, ts)` without resolving a peer.
- **`history` reads one page.** It ignores `has_more`
  (`crates/relay-core/src/slack.rs:193-219`). If more than 200 messages arrived while
  offline, the oldest are skipped and the cursor jumps past them for good.
  Fix: follow `response_metadata.next_cursor`.
- **The middle band is dropped.** A rescreen creates no job and stores no answers
  (`slack.rs:569-572`), and `close_stale_slack_episodes` (`store/mod.rs:2351`) has no
  caller, so "rescreen at close" never happens. Shadow counts also lose `unsure`.
  Fix: store a `rescreen` job row, close stale episodes on a timer, and rescreen on close.
- **Change jobs run immediately.** `route(..., false)` is always called with
  `task_claimed = false`, and `analyze` starts every `Change` job at once (`slack.rs:549`,
  `608`). PLAN 3.2 says to wait for episode close unless the task is claimed.
  Fix: pass the claim state, and queue unclaimed changes until close.
- **Group DMs are captured.** `mpim` is accepted (`crates/relay-core/src/slack.rs:310`),
  but PLAN 3.1 makes group DMs opt-in.
  Fix: accept `im` only unless a setting enables group DMs.
- **"At most one new task" is not enforced.** `tasks_create` accepts any number of op
  indexes (`analyst.rs:232-247`).
  Fix: refuse a second `tasks_create` per job.
- **Runner state handling.**
  - `RUNNING` stays `true` if `run` panics (`analyst.rs:27-33`).
    Fix: reset it in a drop guard.
  - `finish` overwrites `superseded` with `failed` (`analyst.rs:45-56`).
    Fix: fail only a job that is `running` or `pending_agent`.
- **The child joins the Job Object after spawn.** `cmd /C` can start `opencode` before
  `AssignProcessToJobObject`, and its return value is ignored (`analyst.rs:129-133`,
  `243-249`).
  Fix: spawn suspended, or check the assignment and log a failure.
- **The analyst token sits in plain text.** It is written to `analyst/opencode.json` and
  stays there after the run (`analyst.rs:91-92`).
  Fix: delete the file when the run ends.
- **The command line goes through `cmd /C`** (`analyst.rs:101-116`). The job id and prompt
  are internal ULIDs and fixed text, and `job_retry` loads the job before spawning, so I
  found no injection path today. The `cmd` wrapper makes any future free-text argument
  injectable.
  Fix: resolve `opencode.cmd` and pass arguments without `cmd /C`, or assert that the job
  id is `[0-9A-Z]{26}`.

### Not covered

- No live Slack workspace was tested. Socket Mode, `hello` catch-up, user-token DM delivery
  and real `message_changed` / `message_deleted` payloads are unverified.
- The reviewer was checked only through the one simulated-DM run log above. Neither the
  timeout kill nor the Job Object was exercised.
- The keyring was not tested across a reboot.
- `cargo test` and `clippy` were not re-run for this review.
- The `8016700` commit boundary (UI files swept in) was not re-audited.


## Follow-up fixes (2026-11-17, after the verdict)

**Commits:** `ee8f735`, `dd53333`, `b505dad` (+ the poll fix in the git record).

- **Peer and direction:** capture now takes the peer from the DM channel
  (`list_dms` / `conversations.info`), not the message author; episode reuse is keyed by
  `channel_id`, and `direction` compares the author with that peer. The account owner's
  replies no longer create a "self" peer or merge across DMs.
- **Reviewer isolation:** the child runs `opencode --standalone`, with `XDG_CONFIG_HOME`
  pointed at an empty directory under the analyst folder, so the user's global config
  (Executor) is not merged. The config now uses the V2 `mcp.servers` / `agents` shapes and
  a catch-all `deny` with one `allow` for `relay-analyst_*`. Two probes with the strict
  config showed only `relay-analyst_jobs_get` and no Executor calls; the earlier full run
  also created a task and a proposal. A log tripwire fails the job if `executor`, `hopper`
  or `spawner` appears.
- **Analyst writes:** they require the job to be `running`; a negative op index is rejected
  before any write; a replay checks that the recorded `tool` matches.
- **Re-analysis:** `create_job` returns `(job, inserted)`; the analyzer returns early when
  the watermark already has a job, so an edit or a redelivery does not reset or rerun it.
- **Retention:** a six-hour loop prunes messages older than 30 days by Slack `ts` and closes
  stale episodes; `slack_clear_capture` also deletes the reviewer run logs.
- **Pause:** catch-up returns before listing DMs and before each channel while capture is
  paused.
- **Still open:** the write and its `job_ops` row are two transactions. A crash in that
  window can duplicate one write on retry; there is no data corruption otherwise. Fixing it
  needs store-level write+op transactions.
