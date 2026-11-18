# Claude Slack proxy transport

**Status:** final
**Agent / model:** Claude Sonnet (implementation); orchestrated and checked by Claude Sonnet 5.5
**Commit(s):** `2e2da6c` (shared with `2026-11-18-github-gh-sources.md`)
**Files:** `crates/relay-core/src/claude_slack.rs`, `crates/relay-core/src/lib.rs`, `src-tauri/src/slack/claude.rs`,
`src-tauri/src/slack.rs`, `src-tauri/src/lib.rs`, `src/app/Slack.tsx`, `src/ipc.ts`

## What changed

- `relay_core::claude_slack` computes the Slack read calls Relay wants and parses stream-json tool results. It owns no
  process and no Tauri types.
- `src-tauri/src/slack/claude.rs` runs headless `claude -p --model haiku` to execute those calls through the claude.ai
  Slack connector, then feeds the result to the same ingestion and JEV path as the app-token transport. It ingests 1:1
  and group DMs where the user sent messages.
- Triggers: a 10-minute schedule (first run 20 s after start), `Ctrl+Alt+S`, the tray item and **Sync now**. All share
  the guard with the app-token transport.
- Settings → Slack has a transport picker (Claude connector or app token), a status line with last run and today's
  cost, and the run error inline. With no saved choice and no Slack token, Claude is the default when `claude` exists.
- Real smoke earlier (by the implementer): 5 channels, 219 messages, $0.045, 53 s.

## Commands + results

Same run as `2026-11-18-github-gh-sources.md`: 88 + 26 tests, clippy, fmt, typecheck, 14 vitest, build all pass.
Screenshot `scratchpad/relay-shots/02-slack.png` shows the picker "Claude (claude.ai Slack connector)", the Claude panel
and its copy, and the inline red error.

## Known risks

- **A real sync ran during the Settings check.** The debug exe uses the real app data directory (Tauri ignores an
  `APPDATA` override), and the default transport is Claude, so the 20 s schedule fired. Three runs happened in about
  ten minutes: the first stored 245 messages, the next two failed. Today's cost shown in Settings was $0.058. This wrote
  to the real `relay.db`. Nothing else was triggered by hand.
- **"Connector is not connected (pending)" after a good run.** The second and third headless runs reported the
  claude.ai Slack connector as `pending` and returned 0 messages, so Sources turned red. The first run connected. This
  looks like a startup race of the connector in a headless session; the code has no retry. Consider one retry after a
  few seconds before marking the source down.
- The schedule starts on app start and costs about $0.015 to $0.045 per run; a user who does not want it must pick the
  app-token transport.
- The prompt and tool names depend on the claude.ai connector's tool surface; a rename breaks parsing (covered by the
  parser's unit tests with fixed fixtures only).

## Review checklist

- Does the shared guard prevent a schedule run overlapping a hotkey run, and release on error and timeout (90 s)?
- Is the `pending` race retried or surfaced accurately? Should it count as "down"?
- Are messages deduplicated against the app-token transport if the user switches transports?
- Is the user's own message detection correct for group DMs?
- Does the cost line reflect only Relay's runs?

## Opus verdict
