# On-demand Slack thread fetch via headless Haiku

Resolves wayfinder issue #9 (map: #6). Researched 2026-11-20 on Windows 11, Claude Code 2.1.329.

## Answer

**Yes, it works and is correct, but it is not "a few seconds".** The existing `claude -p --model haiku` path in
`crates/relay-core/src/claude_slack.rs` can fetch exactly one thread with one `slack_read_thread` call, and the
code for that call already exists (`ToolCall::read_thread`, `parse_thread`). Measured end to end on a cold start:

- **Latency: 6.6-9.5 s wall for a single thread read**, about $0.002 per fetch. Roughly 5 s of that is fixed CLI and
  connector startup; only 2.5-4 s is the model's two turns (call, then "done"). That is "tolerable with a spinner",
  not "interactive". Do not promise sub-3 s.
- **`pending` is explained and already fixed in code.** By default Claude Code connects remote servers in the
  background, so a headless run starts before the claude.ai Slack connector is up: I reproduced `pending`, an empty
  tool list and no tool call in 2 of 2 runs with the default. `MCP_CONNECTION_NONBLOCKING=false` (already in
  `claude_slack::RUN_ENV`) makes startup wait and every blocking run connected. The "pending, no retry yet" note in
  `docs/HANDOFF.md` predates or misses that.
- **Reliable enough with one retry.** Recorded production runs: every run that reached the connector executed its
  calls; failures worth handling are `pending`/timeout (retry once), a skipped call (retry once), and Slack errors
  such as `channel_not_found` (do not retry, tell the user).
- **Parsing a permalink needs no model.** Relay computes `channel_id` and `message_ts` locally and passes them to the
  tool; the model only executes the call (this is how the code already works).

Recommended shape, cheapest first:

1. Parse the permalink locally. Look the thread up in the local `messages` table first: scheduled sync already
   ingests DM threads, so for DMs the fetch is often unnecessary and instant.
2. On a miss, one headless run with one `slack_read_thread` call and a short timeout (30 s, not the sync's 90 s),
   with a visible "fetching" state.
3. Retry once for `pending`, a timeout or a skipped call; never for `channel_not_found` or a signed-out connector.
4. If the budget is really "a few seconds", the headless path cannot meet it; that needs the direct token path
   (`docs/SLACK_SETUP.md`) or a warm long-lived process (both untested here, see open questions).

## Evidence

### Existing code (what already exists)

- `crates/relay-core/src/claude_slack.rs`: `Tool::ReadThread` is `mcp__claude_ai_Slack__slack_read_thread`, arguments
  `{ "channel_id", "message_ts" }` (`ToolCall::read_thread`). `cli_args` runs `-p --model haiku --effort low
  --output-format stream-json --verbose --tools "" --max-turns N --allowedTools <slack tools>` with
  `--settings` turning hooks and plugins off and allowing only https MCP servers.
- `parse_stream` reads the raw `tool_result` from the stream, so the model's reply text is irrelevant; it rejects the
  whole run if the model calls an unrequested tool or arguments, and maps the init event's connector status
  (`connected` / anything else) to `ClaudeError::ConnectorDown`.
- `parse_thread` returns **replies only** (it skips the `=== THREAD PARENT` block). An on-demand "show me this
  thread" needs the parent as well, so it needs a variant, not a new tool.
- `src-tauri/src/slack/claude.rs`: `RUN_TIMEOUT = 90 s`; the child is killed on timeout; `sync` takes `begin_sync()`,
  the same guard as the token transport. A scheduled sync runs 20-70 s, so an on-demand fetch would queue behind it
  unless it gets its own guard.
- `ATTEMPTS = 3` re-runs only calls the model skipped; there is no retry for `pending`, a timeout or a transient
  error.

### Latency and cost, measured

Relay's own argument set, one call per run, `MCP_CONNECTION_NONBLOCKING=false`. Only sizes and timings recorded.

| Run | Wall | CLI-reported duration | Turns | Cost | Result shape |
| --- | --- | --- | --- | --- | --- |
| `slack_read_user_profile` | 7.5 s | 2.6 s | 2 | $0.0109 (cold cache) | JSON, 299 chars |
| `slack_read_thread`, bogus channel id | 8.8 s | 4.0 s | 2 | $0.0024 | `is_error: true`, plain text `execution_failed: channel_not_found` |
| `slack_read_thread`, real thread, parent ts | 9.5 s | 3.6 s | 2 | $0.0024 | JSON, 2 message blocks incl. parent |
| `slack_read_thread`, same thread, a reply's ts | 6.6 s | 2.5 s | 2 | $0.0023 | JSON, 1 message block |
| profile, default (non-blocking) startup, x2 | 3.8 s, 3.9 s | 1.5-1.6 s | 1 | $0.0002-0.0008 | Slack `pending`, 0 tools, no call |

Wall minus the CLI-reported duration is about 5 s, which is process start plus waiting for the connectors. The
production run log (`claude_slack.runs`, 50 kept) agrees: single-call runs took 7.3 s and 11.5 s at $0.0014; full
syncs of 5-7 calls in 3-4 invocations took 21-70 s.

Observations that matter for the design:

- The non-blocking default is faster (about 4 s) but useless: it returns before the connector exists.
- Passing a **reply's** ts returned one block where the parent ts returned two. Use the parent ts (the permalink's
  `thread_ts`), as the sync already does.
- 26 consecutive earlier runs in the log failed with "the model did not run the requested call" (3 invocations,
  12-18 s, 0 calls, $0). The log does not say why and I could not reproduce it; the later runs are clean. Treat
  "skipped call" as a real, retryable failure.

### Failure modes

| Failure | What it looks like | Retry? |
| --- | --- | --- |
| Connector `pending` | init `mcp_servers` status `pending`, empty tool list, no call, run still exits 0 | Yes once; avoided by `MCP_CONNECTION_NONBLOCKING=false` |
| Connector signed out | status `needs-auth`; headless cannot re-authenticate | No; docs say sign in via claude.ai, then start the run again |
| First connection fails (5xx, refused, timeout) | Claude Code itself retries remote servers up to 3 times, then marks `failed` | One app-level retry at most |
| Model skips or alters the call | `None` result, or `Rejected` for unrequested arguments | Skipped: yes once; rejected: no |
| Channel missing or no access (private channel, other people's DMs) | `execution_failed: channel_not_found`, `is_error: true`, not JSON | No; message the user |
| Timeout | child killed after `RUN_TIMEOUT` | Once, with a shorter budget |
| Rate limit | Not observed in 50 recorded runs; see open questions | Back off per `Retry-After` if the tool surfaces it |
| Claude Code missing or not signed in | `ClaudeError::NotFound` / run prints the failure as the result | No |

Private channels and permissions: the connector acts as the signed-in user, so a thread is readable exactly when
the user can read it in Slack. The bogus-id run shows the not-found shape; Slack's API uses `channel_not_found` for
an invalid channel value (<https://docs.slack.dev/reference/methods/conversations.replies>), so "no access" and
"does not exist" are probably indistinguishable. The user-facing message should say "not found or not visible to
you".

### Permalink to channel and ts (Slack API docs)

From `chat.getPermalink` (<https://docs.slack.dev/reference/methods/chat.getPermalink>):

- Plain message: `https://<host>/archives/<CHANNEL>/p<digits>` where `p<digits>` is the `ts` with the dot removed
  (doc example `p135854651500008` for ts `1358546515.000008`).
- Message inside a thread: `.../archives/<CHANNEL>/p<reply digits>?thread_ts=<parent ts>&cid=<CHANNEL>`; the path
  digits are the reply, the query parameter is the **parent**.

Parsing rule: `channel_id` is the segment after `/archives/`; `ts` is the path digits with a dot inserted before
the last 6 digits; if `thread_ts` is present use it as `message_ts`, else use `ts`. Hosts vary (workspace
subdomains, enterprise grid hosts, `app.slack.com/client/...`); only the `/archives/` form is documented in the
sources above, so reject other forms with a clear message instead of guessing.

### Claude Code CLI facts used

- Connectors from claude.ai are available to a signed-in Claude Code automatically
  (<https://code.claude.com/docs/en/mcp>, "Use MCP servers from claude.ai").
- Other servers connect in the background by default; `MCP_CONNECTION_NONBLOCKING=0` makes startup wait for them
  (same page). This is the documented cause of `pending`.
- Remote first-connection failures retry up to three times, then the server is `failed`; "to reconnect it in
  `claude -p`, start the run again". `needs-auth` needs a sign-in outside the run (same page).
- Per-request timer for claude.ai connectors is the greatest of 60 s, the tool timeout and `MCP_TIMEOUT` (same
  page), so a slow Slack call can outlive a short Relay timeout; the child kill is the real bound.
- `--bare` is the documented fast-start mode but "never reads OAuth credentials or the system keychain" and skips
  MCP auto-discovery (<https://code.claude.com/docs/en/headless>), so it cannot reach the claude.ai connector.
- `--output-format stream-json --verbose`, `--allowedTools`, `--max-turns` are as the code already uses them (same
  page).
- Slack API limits for the equivalent direct call: `conversations.replies` is Tier 3 (50+/min), and a 429 carries
  `Retry-After` (<https://docs.slack.dev/apis/web-api/rate-limits>).

## Open questions

- **Can latency drop below 5 s?** Candidates, none tested: keep one long-lived `claude -p --input-format
  stream-json` process warm so only the first fetch pays startup; stop reading at the first `tool_result` and kill
  the child instead of waiting for the "done" turn (saves the second turn, about 1.5-2.5 s); drop `--effort`/prompt
  overhead. Worth one prototype before committing to a UX.
- **Does the connector rate-limit or throttle `slack_read_thread`?** Unknown; the connector is not the raw Web API
  and its limits are not documented in the sources above. A burst test was out of scope.
- **Scopes of the connector for private channels and group DMs.** The thread read worked for a DM thread; private
  channel and group DM reads were not tested with real data.
- **Does `slack_read_thread` ever paginate long threads?** The tool output carried no pagination marker for the
  short thread tested.
- **Subscription usage limits** also apply to headless runs; cost per fetch is tiny but the budget impact of many
  on-demand fetches was not measured.
- **Cause of the 26 early "skipped call" runs** in the run log is not established.
