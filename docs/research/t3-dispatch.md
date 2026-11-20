# How Relay dispatches a Task to T3 Code

Ticket: uFleck/relay#8. Map: uFleck/relay#6.

Source studied: the open-source repo `pingdotgg/t3code` (MIT), commit
[`8d37736`](https://github.com/pingdotgg/t3code/tree/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d)
(2026-11-20, server/desktop package version 0.1.11). Every link below pins that commit. T3 Code changes
fast, so re-check the facts marked "version-sensitive" before building on them.

## Answer

T3 Code has a supported, documented way for an outside local process to start and track an agent thread.
There are four surfaces; two are usable for dispatch.

| Surface | Can start a thread with a brief? | Can track it? | Auth |
| --- | --- | --- | --- |
| **`t3 thread` CLI** (`start`, `send`, `show`, `list`) | Yes | Yes (poll `show --json`, or block with `--wait`) | None for the caller: it must run on the same machine, as the same OS user, with the same T3 data directory |
| **MCP endpoint `<origin>/mcp`** (HTTP, OAuth) | Yes (`t3_thread_launch`) | Yes (`t3_thread_read`, `t3_thread_wait`, `t3_thread_list`) | One-time OAuth approval in T3, 30-day session, read-only or a permission-mode ceiling |
| WebSocket RPC (the app's own protocol) | Yes, but not reachable by an MCP-style client | Yes, with event streaming | A fully paired client session (`t3 auth pairing create`); `mcp-client` sessions are rejected there |
| Prompt link `https://app.t3.codes/new?prompt=...&repo=...` | Draft only: the user must press Send | No | None |

**Recommendation for the Relay contract.**

1. Primary path: shell out to `t3 thread` from Rust. `t3 thread start <project> "<brief>" --json
   [--worktree <base-ref> --branch <name>] [--provider <id> --model <slug>]` returns the thread id and run
   id; `t3 thread show <id> --json` returns status, branch, worktree path, linked pull requests, pending
   questions and the latest messages. Relay stores no T3 secret, because the CLI signs itself in from the
   T3 data directory on disk.
2. If Relay needs more than the CLI exposes (idempotent retries, an explicit permission mode, attachments,
   structured wait, answering a pending question, linking a PR), Relay becomes an MCP client of
   `<origin>/mcp` and does the OAuth approval once. That is a heavier build (OAuth with PKCE and a loopback
   redirect) and is the only route to the full tool surface.
3. Fallback when T3 is not running or the CLI is missing: open a prompt link so the user can send the draft
   themselves. Relay then has no thread id and cannot track anything.

What can be observed afterwards: thread id, per-run status, the final reply or the error text, the branch
and worktree path, linked pull requests with their state, and whether the thread is blocked on a human
answer. What cannot: there is no push or event stream for an outside caller (polling or a blocking wait
only), a PR is linked only if the agent creates it in a way T3 recognises or calls a link tool, and
"completed" means the run ended, not that the work is right or the PR merged.

## Evidence

### 1. `t3 thread` CLI (documented for outside use)

- User docs, "Scripts on the host":
  [`docs/user/outside-agents.md`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/docs/user/outside-agents.md).
  It states `t3 thread` "uses the same tools without a sign-in" on the machine that runs the environment,
  and shows `start`, `send ... --wait`, `show`, `list`.
- Implementation:
  [`apps/server/src/cli/thread.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/server/src/cli/thread.ts)
  (added by PR #22732, 2026-10-21, so older installed versions will not have it; version-sensitive).
  - How it connects (`withThreadTools`, lines 219-261): reads `server-runtime.json` from the T3 state
    directory to find the running server's origin, mints a short-lived MCP client session straight from
    the local auth store (`issueMcpClientSession`, access `full-access`), opens an MCP session to
    `<origin>/mcp` with that bearer token, and revokes the session when the command ends. So the CLI is
    a thin wrapper over the same MCP tools an outside agent uses. If no live server record exists it
    fails with "No running T3 Code server uses <dir>. Start T3 Code, then retry."
  - `start` (line 829): calls `t3_thread_launch` with `projectId`, `title` (defaults to the brief's first
    line, 80 chars), `message`, optional `workspaceStrategy` (`--worktree <base-ref>` and `--branch`),
    optional model selection (`--provider` and `--model` together, plus `--option id=value`). Output is the
    thread id, or with `--json` the launch result (`threadId`, `link`, `projectId`, `modelSelection`,
    `runId`, `status`). The brief can be piped on stdin with `-`.
  - `--wait` (lines 666-701): blocks until the run ends and prints `{ threadId, runId, status, reply,
    error }` with `--json`. Exit code 1 when the run fails, is cancelled or is interrupted; exit code 2 when
    the agent is waiting for an answer that only the T3 app can give (`ThreadNeedsInputError`, line 108).
  - `show --json` (line 947): prints `{ thread, messages, error }` where `thread` is the full detail
    object (see section 3).
  - There is no flag for runtime/permission mode, `clientRequestId` (so a retried `start` creates a second
    thread), attachments, or pull-request linking.
- Project resolution (`resolveProject`, line 318): the first argument is a T3 project id or a path inside a
  project's folder (deepest project wins; a path in a git worktree of the repo also resolves). If no
  registered project contains the path, the command fails with `ThreadProjectNotFoundError`. The CLI can
  register one: `t3 project add <workspace-root>`
  ([`cli/project.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/server/src/cli/project.ts),
  line 351).
- Finding the binary: the desktop app installs a `t3` launcher on first launch (a `t3.cmd` shim on Windows)
  into a user bin folder, adds that folder to the user PATH, and the shim sets `T3CODE_HOME` to the app's
  data directory
  ([`DesktopCliShim.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/desktop/src/app/DesktopCliShim.ts),
  [`DesktopCliCommand.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/desktop/src/app/DesktopCliCommand.ts)).
  Removing it in Settings sticks. Relay should probe PATH for `t3`, and also respect `T3CODE_HOME` if set.
- State location: the data directory defaults under the user's home; state files live in its `userdata`
  folder, including `server-runtime.json` (fields: `pid`, `port`, `origin`, `startedAt`, `serverVersion`)
  ([`serverRuntimeState.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/server/src/serverRuntimeState.ts),
  [`config.ts` `deriveServerPaths`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/server/src/config.ts)).
  The default port constant is 3773, but the real port is read from the runtime file, so Relay should not
  hardcode it.

### 2. MCP endpoint (the surface the in-thread orchestration tools use)

- Design doc:
  [`docs/orchestration-v2/orchestrator-mcp-server.md`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/docs/orchestration-v2/orchestrator-mcp-server.md).
  The orchestration tools share the existing authenticated HTTP MCP endpoint `http://127.0.0.1:<port>/mcp`
  (provider-visible server key `t3-code`). Agents T3 launches get a per-provider-session bearer token that
  is scoped to their thread, expires, and is revoked when the session ends. That credential is not
  available to an outside process.
- Outside access is explicitly supported, same doc and
  [`outside-agents.md`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/docs/user/outside-agents.md):
  "Agents that T3 Code did not start can work with an environment through its MCP server." Documented
  clients are Claude Code (`claude mcp add --transport http t3 <url>` then `claude mcp login t3`), Codex,
  ChatGPT, and any remote-MCP client with OAuth. Settings has "Copy MCP URL" (the environment address plus
  `/mcp`).
- Transport: streamable HTTP MCP. The server expects an `mcp-protocol-version` header on follow-up requests
  (a bridge note in the design doc says 2025-06-18 and that the transport answers 400 without it).
  Implementation:
  [`apps/server/src/mcp/McpHttpServer.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/server/src/mcp/McpHttpServer.ts);
  a 401 carries `resource_metadata` pointing at `/.well-known/oauth-protected-resource/mcp`.
- Auth for outside clients:
  [`docs/internals/environment-auth.md`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/docs/internals/environment-auth.md)
  ("MCP clients are a separate audience"). A narrow OAuth authorization-code server
  ([`auth/McpOAuth.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/server/src/auth/McpOAuth.ts)):
  loopback redirect URIs for agents on the user's machine, HTTPS redirects for hosted agents, every client
  public with PKCE, stateless client registration, and no client id or secret needed. Approval spends a
  one-time pairing code or uses a signed-in browser session with access-management permission. The user
  picks **read-only** or a **runtime-mode ceiling** (Supervised through Full access); a thread the client
  starts cannot run with more permission than that ceiling. A sign-in lasts 30 days, then the user approves
  again. The user can revoke it in Settings, Sharing and access. Plain `http` is refused for sign-in except
  to `localhost`.
- `mcp-client` sessions are accepted only on `/mcp`; every other HTTP and WebSocket route rejects them.
- Tools an outside client can use for dispatch and tracking (full list in
  [`mcp/toolkits/orchestrator/tools.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/server/src/mcp/toolkits/orchestrator/tools.ts)
  and
  [`mcp/toolkits/project/tools.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/server/src/mcp/toolkits/project/tools.ts)):
  - `t3_project_list`, `t3_project_read`, `t3_project_create` (register a folder), `t3_project_clone`.
  - `t3_thread_launch` (project tool file, line 130): `projectId` or `scratch`, `title`, `modelSelection`,
    `runtimeMode`, `interactionMode`, `workspaceStrategy` (`root` default, `worktree` with `baseRef`,
    `branch`, `startFromOrigin`, or `existing_worktree`), `message` (up to 120,000 chars), up to 8
    `attachments`, `clientRequestId`. It returns `threadId`, `link`, `projectId`, `modelSelection`, `runId`,
    `status`. Retrying with the same `clientRequestId` returns the same work (idempotent).
  - `t3_thread_list`, `t3_thread_read`, `t3_thread_wait`, `t3_thread_send`, `t3_thread_interrupt`,
    `t3_thread_search`, and for pull requests `link_pull_request`, `unlink_pull_request`,
    `list_thread_pull_requests`, `watch_pull_request`.
  - Tools that act as a calling thread (subagents such as `delegate_task`, `create_threads`, secret
    requests, HTML render, preview, devices, worktree handoff) appear in the list but fail for an outside
    client with an error that says they need an agent running inside T3 Code. `outside-agents.md`
    says to use `t3_thread_launch` instead, and to pass `projectId` or `threadId` explicitly.
- Reaching the server from another machine or a cloud agent is a separate setup (T3 Connect or Tailscale
  HTTPS); it is irrelevant to a local desktop app, which can use loopback.

### 3. What the observation tools return

Schemas:
[`packages/contracts/src/orchestratorMcp.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/packages/contracts/src/orchestratorMcp.ts)
(lines 322-627).

- Run status values (`OrchestrationV2RunStatus` in `orchestrationV2.ts`, line 682): `preparing`, `queued`,
  `starting`, `running`, `waiting`, `completed`, `interrupted`, `failed`, `cancelled`, `rolled_back`.
  A thread's status is that or `idle`.
- `t3_thread_wait` (design doc): waits for a run to reach `completed`, `failed`, `cancelled`,
  `interrupted` or `rolled_back`. A timeout reports the latest durable status and does not cancel the work.
  An idle thread returns immediately.
- Thread list item and thread detail include: `threadId`, `projectId`, `title`, `link` (an
  `https://app.t3.codes/threads/<environmentId>/<threadId>` URL for outside clients), `status`,
  `latestRunId`, `providerInstanceId`, `model`, `runtimeMode`, `interactionMode`, `linkedPullRequest`,
  `pullRequests[]` (each with `url`, `source`, `state`, `watching`), `pullRequestCount`,
  `openPullRequestCount`, `settled` and `settledAt`, `snoozed`, `createdBy` and `creationSource`
  (`agent` and `mcp` for MCP-created threads), timestamps. Detail also has `branch`, `worktreePath`,
  `pendingRequestCount`, `runCount`, `itemCount`, `archived`, `pendingBackgroundTasks`.
- `t3_thread_read`: recent runs (`status`, `startedAt`, `completedAt`, executed model) and a timeline of
  messages (view `messages`) or also tool, reasoning and runtime-request items (view `activity`). Output
  is bounded (about 30,000 characters per result) and paginated by `afterPosition` and `nextPosition`.
  Outside clients read from the oldest item forward.
- Failure detail: `failed` runs expose the provider error; the CLI fetches it from the thread's `error`
  item (`runResultText(..., "error")`, `thread.ts`).
- Pull request linking
  ([`docs/user/source-control.md`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/docs/user/source-control.md),
  "Linked pull requests"): a PR is linked to the thread automatically when created from T3's Git actions,
  or when the agent creates it with `gh pr create` or `glab mr create` (the server recognises that command
  in the agent's shell:
  [`PullRequestSyncReactor.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/server/src/orchestration-v2/PullRequestSyncReactor.ts)).
  Other PRs need `link_pull_request`. That tool takes an explicit `threadId`, so an outside MCP client with
  a non-read-only grant can link a PR itself (`source: "agent"`;
  [`pullRequests/tools.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/server/src/mcp/toolkits/pullRequests/tools.ts),
  line 60). Linked PRs are refreshed on the server (open ones continually, quiet ones every 5 to 15
  minutes), and with the "auto-settle merged threads" setting a thread settles when every linked PR is
  terminal.
- Answering a blocked agent: when an agent asks a question or needs an approval, the run goes to `waiting`
  and `pendingRequestCount` is above zero. The CLI cannot answer (exit code 2). The MCP tool
  `t3_pending_request` has `list`, `read` and `respond` actions
  ([`thread/tools.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/apps/server/src/mcp/toolkits/thread/tools.ts),
  line 129; access rules in `thread/handlers.ts`, line 248). `list` and `read` count as reads, so a
  read-only grant can use them; `respond` is a write checked against the grant's permission ceiling and
  needs no calling thread. It answers user questions only: approvals are not listed and the tool "cannot
  approve a permission request", so a thread stuck on a command approval still needs the T3 app.

### 4. Prompt links (draft only)

[`docs/user/prompt-links.md`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/docs/user/prompt-links.md):
`https://app.t3.codes/new?prompt=...&repo=owner/name&provider=...&model=...` (also `pr=owner/name/123`).
"A link never runs anything. It opens a draft. You read the prompt, then press Send." It cannot set
permission mode, branch or worktree, the prompt is capped at 8,000 characters, and `repo` is a GitHub
repository matched to the user's most recent local project for it. The desktop installer also registers a
`t3code` URL scheme
([`scripts/build-desktop-artifact.ts`](https://github.com/pingdotgg/t3code/blob/8d37736cddce74e7cc7ff965b26b1d3ea1fbcd7d/scripts/build-desktop-artifact.ts),
line 2871); I found no doc for a `t3code://` prompt link, so only the `https` form is documented. A link
gives Relay no thread id, so nothing can be tracked.

### 5. Why not the WebSocket RPC

The app's own client protocol requires a fully paired client session (`t3 auth pairing create`, or
`t3 auth session issue`) whose default grant includes terminal access and access management
(`environment-auth.md`, "Authority survives transport changes"). Handing a desktop helper that much
authority is the wrong trade when the CLI and MCP routes cover dispatch. MCP-client sessions, which are
the narrow ones, are rejected on that route by design.

## Facts later decisions depend on

- **T3 must be running.** Both routes need a live T3 server on the same machine. The CLI reports "No running
  T3 Code server" otherwise. Relay cannot start T3 through any documented surface; "background service"
  mode exists (`docs/user/background-service.md`) but I did not check how it behaves on Windows.
- **The repo must be a registered T3 project.** A Task's repo maps to a T3 project by folder (or id).
  Relay must handle "not registered" (offer `t3 project add <root>` or the MCP `t3_project_create`).
- **Workspace isolation is the caller's choice and defaults to sharing the user's checkout.** The default
  `root` strategy runs the agent in the project folder itself; `--worktree <base-ref>` (CLI) or
  `workspaceStrategy: worktree` (MCP) creates a separate checkout. Uncommitted edits are never copied.
  Relay should likely default Task dispatch to a worktree.
- **Permission mode is not settable from the CLI.** A CLI-started thread uses the project's default model
  and permission mode. Over MCP, `runtimeMode` can be set but never above the grant's ceiling. A thread
  that needs approvals will stop at `waiting`.
- **No push notifications for outside callers.** Relay polls (`show --json` every N seconds, or a blocking
  `--wait` child process, which also tells "needs a human" with exit code 2 apart from failure with exit
  code 1). A long-lived `--wait` process per Task is simple but holds a process per dispatched Task.
- **Idempotency.** The CLI `start` has no request id; a crash between launch and persisting the thread id
  can leave an orphan thread. Over MCP, `clientRequestId` makes retries safe. Relay can reduce the window
  by storing the intended dispatch before calling and listing recent threads by title on recovery.
- **Credentials stay out of Relay on the CLI route.** On the MCP route Relay must store the OAuth token
  (30-day session) in Windows Credential Manager, consistent with the repo rules.
- **Version drift.** The CLI and tool schemas are young (the CLI was added in late October 2026; the CLI
  prints "Update the t3 CLI to match the server" on a result it cannot decode). Relay should read
  `serverVersion` from `server-runtime.json` and degrade to the prompt link when too old.

## Open questions

1. Should Relay answer agent questions itself (MCP `t3_pending_request respond`) or only surface "needs
   input" and link the user to the thread? The tool exists for outside clients, but approvals cannot be
   given through it.
2. Does the Windows desktop install actually put `t3.cmd` on PATH for a GUI-launched process such as
   Relay? The code writes the shim and updates the user PATH in the registry, but a running app only sees
   a changed PATH after restart. Probe the shim path directly as well as PATH.
3. Behaviour of the "background service" mode on Windows: does T3 keep a server running when the desktop
   window is closed, so that a dispatch can succeed with the app closed?
4. Should Relay hold one persistent MCP client (OAuth) instead of using the CLI? It buys idempotency,
   permission mode, structured waits and pending-request answers at the cost of an OAuth client and token
   storage; the ticket that specifies the dispatch contract should decide.
5. Which signal means "done" for a Task: run `completed`, a linked PR, a merged PR, or the thread
   `settled`? T3 exposes all four, they mean different things, and only the PR state is host-verified.
6. Is the OAuth redirect from a Tauri app (loopback listener) acceptable to the approval page? The docs
   allow loopback redirects for agents on the user's machine; I did not run a flow.

## Not covered

Nothing was executed against a live T3 instance; everything here comes from reading source and docs at
the pinned commit. Provider-specific behaviour (which models and providers are available) and the
mobile or T3 Connect paths were not investigated beyond what affects a local desktop caller.
