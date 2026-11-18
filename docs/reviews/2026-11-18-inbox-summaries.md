# Inbox row summaries

**Status:** final
**Agent / model:** Claude Opus 5.5
**Commit(s):** branch `t3/ae15a2c5`, on top of `8aec3c5` (v0.6.1)
**Files:** `crates/relay-core/migrations/0004_conversation_summaries.sql`, `crates/relay-core/src/summaries.rs`,
`crates/relay-core/src/store/mod.rs`, `crates/relay-core/src/lib.rs`, `src-tauri/src/slack/claude.rs`,
`src/app/Slack.tsx`

## What changed

- Each Inbox row shows `conversations.summary`, a one-line AI label of what the conversation is about. Until an episode
  has a summary, the row shows its last message, as before. The tooltip shows the last message.
- Migration 0004 adds `conversations.summary_through`: the `last_message_at` value that the summary covers. A newer
  message marks the summary stale.
- `relay_core::summaries` holds the policy. It selects episodes with text whose summary is missing, or whose summary is
  stale and that have been quiet for 5 minutes, newest first, 12 per pass. It builds the prompt from the last 40
  messages (400 characters each) and parses and cleans the reply. A per-episode failure skips that episode; a failure
  of `claude` itself ends the pass.
- The AI path is the one the Claude Slack sync uses: headless `claude -p`, haiku, low effort, and the same
  `ProcessRunner` (timeout, no console window). The run has no tools (`--tools ""`), no MCP servers
  (`--strict-mcp-config`, `ENABLE_CLAUDEAI_MCP_SERVERS=false`), no hooks or plugins, and one turn.
- `summarize_in_background` runs passes with `spawn_blocking`, never more than one at a time (an `AtomicBool`). It runs
  30 s after start and every 3 min after that, for either Slack transport, and after each Claude Slack sync. It loops
  while a pass fills its quota, which is how old episodes get backfilled. It emits `relay://change` when a pass writes a
  summary. It skips while capture is paused or when `claude` is missing.

## Measurements

One real run with the exact arguments, on a 3-message Portuguese transcript ending in "faz sentido":
`Deploy do bill quebrado no RC após merge de PR de faturas`, $0.0006, ~7 s, 1 turn, `is_error: false`.

## Checks

`cargo test -p relay-core` (126 passed, 2 ignored; includes 6 summaries tests and a v3→v4 upgrade test),
`cargo fmt --all`, `cargo clippy -p relay-core -p relay` (clean), `cargo build -p relay`, `pnpm typecheck`, `pnpm test`
(14 passed), `pnpm build`: all pass.

## Risks / not covered

- The app was not run end to end. There is no project verify skill; the CLI path was checked by the one direct run
  above.
- The cost is not added to the Claude Slack status (`costTodayUsd`). A backfill of N episodes costs about N × $0.0006.
- The token transport gets summaries only when `claude` is installed. Without it, rows keep the last message.
- An episode where the model keeps returning an empty reply is retried on every pass.

## Checklist

- [x] Policy in relay-core; the Tauri side only schedules and spawns
- [x] Migration through `PRAGMA user_version`, with an upgrade test from v3
- [x] No IPC contract change (`SlackEpisode.summary` already existed)
- [x] Generation off the UI thread (`spawn_blocking`); no new sync command

## Opus verdict

