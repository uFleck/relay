# Claude Slack sync: stop booting local MCP servers

**Status:** final
**Agent / model:** Claude Opus 5.5
**Commit(s):** TBD (the commit that adds this file)
**Files:** `crates/relay-core/src/claude_slack.rs`, `src-tauri/src/slack/claude.rs`, `src-tauri/src/mcp/server.rs`

## What changed

Problem: Relay lagged the machine. Idle, the Relay process tree uses about 0.6% CPU. The spikes come from the Claude
Slack transport. Every 10 minutes it runs `claude -p` 5–7 times in a row (`claude_slack.runs`: about 40 s per sync,
mostly with 0 new messages). Each run started every user-level stdio MCP server: `local-mcp` (`lmcp-server.exe`),
executor, node and powershell children. That cost about 15 CPU-seconds and 9–14 processes per run, and the sync only
calls the claude.ai Slack connector.

- `run_settings_json` adds `allowedMcpServers: [{serverUrl: "https://*"}]`. Remote connectors stay and local stdio
  servers are not started.
- `RUN_ENV` (`MCP_CONNECTION_NONBLOCKING=false`) is set on the spawned CLI. Without the slow local servers the CLI
  starts before the claude.ai connectors connect, and Slack showed `pending` at init, which Relay treats as
  `ConnectorDown`. Blocking startup waits for the connectors. It should also remove the occasional
  "Slack connector is not connected (pending)" runs already in the run history.
- MCP responses send `Content-Type: application/json; charset=utf-8`. Stored text was already correct UTF-8: the DB
  holds `também` as U+00E9. A Windows PowerShell 5 client decodes a body without a charset as Latin-1.

## Checks

Probe: one `claude -p` run that calls `slack_read_user_profile`, measured with psutil over the process tree.

| Variant | Wall | Child CPU | Peak procs | Slack at init | Tool result |
|---|---|---|---|---|---|
| Before | 6.7 s | 15.5 s | 12 | connected | 1 |
| `https://*` allowlist only | 4.9 s | 6.4 s | 4 | pending | 0 |
| Allowlist + `MCP_CONNECTION_NONBLOCKING=false` (×2) | 6.4 / 6.9 s | 6.2 / 6.3 s | 5 | connected | 1 |

`CLAUDE_CODE_MCP_PREWAIT_SERVERS`, `CLAUDE_CODE_MCP_CONNECTOR_PREWAIT_MS` and `CLAUDE_CODE_MCP_STARTUP_WAIT_MS` did not
help: Slack stayed `pending`.

- `cargo test -p relay-core claude_slack`: 18 passed (the settings test now asserts the allowlist).
- `cargo test -p relay --lib mcp`: 18 passed.
- `cargo test -p relay --lib real_smoke -- --ignored --nocapture`, against the real CLI and connector:
  `channels=5 messages=154 episodes=8 calls=11 invocations=7 cost_usd=0.0272 seconds=54.9 error=None`.
- `cargo fmt --check` and `cargo clippy -p relay-core -p relay --all-targets -- -D warnings`: clean.

## Known risks

- `MCP_CONNECTION_NONBLOCKING` is read by Claude Code 2.1.303 but is not a documented flag. A CLI update could drop
  it. If that happens, runs fail with `ConnectorDown(pending)` instead of silently costing more.
- A user remote MCP server on plain `http://` (not https) is also excluded. The sync never calls one.
- The 10-minute interval and 5–7 invocations per sync are unchanged. Each run still costs about 6 CPU-seconds.
- Not measured in the installed app; the numbers come from the probe and the smoke test.

## Review checklist

- Is `https://*` the right allowlist, or should it name the claude.ai connector's URL?
- Should the interval grow when consecutive syncs return 0 messages?

## Opus verdict
