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

## Opus verdict

