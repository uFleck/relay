# Git/PR context, imports and runner hardening

**Status:** backend complete; the two UI slices are `e92e9c6` (git context) and a pending
ClickUp/Issues import UI.
**Agent / model:** DeepSeek V4.1.
**Commits:** `fff648c` (PR sync + git state), `542716f` (ClickUp/Issues import), `220c74e`
(repo path + PR metadata in the snapshot), `d19bf0f` (link lookup test), `d163fe1`
(job requeue + next-queued job), `9064ec9` (on-demand MCP tokens), `0ac6bd7` (import
bindings), `163d5f6` (import pagination).

## What changed

### Git and pull requests (PLAN 3.5)

- `crates/relay-core/src/git.rs`: `state(path)` reads branch, HEAD, subject and dirty via
  `git -C`, never libgit2. A plain folder is `NotARepository`, not a crash.
- `crates/relay-core/src/github.rs`: `pull_request(owner, repo, n)` reads the PR, the head
  commit's check runs and the latest review per author; `list_issues` excludes PRs.
- `projects.repo_path` can be set/cleared (`set_project_repo`); the snapshot now sends
  `repoPath` and each link's `metadataJson`.
- `src-tauri/src/github.rs`: token in Credential Manager (`github-token`, optional), a poll
  every 3 minutes over `kind = "pr"` links, state mapping (open/draft/merged/closed) and
  metadata `{checks, review, headSha}`. Commands: status/set/clear, `sync_prs_now`,
  `project_git_state`, `project_set_repo`.
- The UI (`e92e9c6`) shows the repo, branch/dirty, a task's PR chips and a Settings token.

### ClickUp and GitHub Issues import (PLAN 3.6)

- `relay-core/src/clickup.rs`: one list page (100), raw `Authorization: <token>` (no
  Bearer), status/due/priority/url mapping; `check()` lists teams.
- `github_import_issues` reuses `GithubClient::list_issues`.
- Imports are one-shot and idempotent: `find_link(kind, external_id, url)` skips a task the
  source already owns. A created task gets a link with the source URL and state.
- ClickUp status names map case-insensitively onto the project's statuses, else `Todo`;
  priority clamps to 1..3; due dates convert from Unix milliseconds.
- Commands: ClickUp status/set/clear/import and `github_import_issues`; both emit
  `relay://change`, so the main window refreshes.

### Hardening

- `job_retry` requeues a failed or queued job; after a reviewer run the next
  `pending_agent` job starts automatically.
- `mcp_token(which)` reveals one bearer token on demand; `mcp_status` no longer needs to
  carry them (the UI switch is the next slice).

## Commands and results

| Command | Result |
| --- | --- |
| `cargo test -p relay-core` | **69 passed** |
| `cargo test -p relay --lib` | **22 passed** |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `pnpm typecheck` / `vitest` / `build` | pass |

## Known risks

- No live PR was polled and no real ClickUp list or GitHub issue was imported; only
  mock servers cover the parsers.
- Imports read up to 10 pages of 100 (1000 items); a larger list needs a cursor loop before
  1.0 is truly complete.
- The ClickUp personal token is workspace-wide; the Settings note says so.
- `mcp_status` still returns the tokens (the UI masks them); `mcp_token` exists so the next
  UI slice can stop that.
- The PR poller runs on a fixed 3-minute timer with no backoff on repeated errors; a rate
  limit shows as the status error until the next poll.

## Review checklist

- [ ] Is `git::state` safe on a missing path, a plain folder and a detached HEAD?
- [ ] Does the PR poller update only linked PRs and never invent links?
- [ ] Is an import idempotent on the second run (by external id or URL)?
- [ ] Can an import create a task in a project that does not exist?
- [ ] Does a failed job retry cleanly, and does the next queued job run?

## Opus verdict

## Opus verdict (2026-11-17)

**Verdict:** changes needed
**Reviewed:** `fff648c`, `542716f`, `220c74e`, `d19bf0f`, `d163fe1`, `9064ec9`, `0ac6bd7`,
`163d5f6`, read at those commits and at `163d5f6` (HEAD). Tests were not re-run.

### What holds

- Tokens are stored under keyring service `dev.relay.app` with keys `github-token` and
  `clickup-token` (`src-tauri/src/secrets.rs:4`, `src-tauri/src/imports.rs:15-16`).
  ClickUp sends the raw token with no `Bearer` (`crates/relay-core/src/clickup.rs:109`).
  GitHub uses `bearer_auth` (`crates/relay-core/src/github.rs:203-205`). Both `Debug`
  impls redact the token (`clickup.rs:44-47`, `github.rs:72-75`), and error bodies are cut
  to 200 characters of the response, never the request.
- Nothing is written back. The PR poller only reads links of `kind = "pr"`. It accepts only
  `github.com/<owner>/<repo>/pull/<n>` URLs and updates only those links' state, title and
  metadata (`src-tauri/src/github.rs:62-120`). Imports only issue `GET` requests.
- Pagination is bounded: ClickUp pages 0..10 and stops on a short page
  (`clickup.rs:88-101`); GitHub pages 1..=10 and filters out pull requests
  (`github.rs:182-193`, `222-223`).
- `git::state` passes the path as its own argument after `-C`, never through a shell
  (`crates/relay-core/src/git.rs:73-76`).
- `mcp_token` reads only the two known token files from an allowlist
  (`src-tauri/src/mcp/mod.rs`, `9064ec9`).

### What I would change

Blocking:

1. **GitHub issue links collide across repos.** The import keys an issue by its number
   alone (`src-tauri/src/imports.rs:155`), and `find_link` matches
   `kind = 'github' AND (external_id = ? OR url = ?)` (`store/mod.rs:1517-1523`). After
   importing `acme/a`, an import of `acme/b` skips every issue whose number already exists
   in `a`. The import reports these as skipped and creates nothing.
   Fix: use `external_id = "<owner>/<repo>#<n>"`, or match on the URL only.
2. **One failing PR stops the whole poll.** An error returns from the loop at
   `src-tauri/src/github.rs:98`. A deleted, private or renamed PR therefore stops every
   later link from syncing on every poll.
   Fix: record the error on that link (`set_link_sync` with an error state), continue the
   loop, and report a count of failed links.

Should fix before 1.0:

- **Task and link are written in separate calls.** `create_task` and `add_link` run
  separately (`imports.rs:111-124`, `168-181`). A failure between them, or two concurrent
  imports, leaves an unlinked task, and the next import duplicates it.
  Fix: one store method that creates the task and the link in one transaction.
- **The 10-page cap truncates silently** (`clickup.rs:88`, `github.rs:182`). Lists larger
  than 1,000 items import partially with no warning.
  Fix: return a `truncated` flag in `ImportCount`.
- **ClickUp 429 and `Retry-After` are not honoured,** although PLAN 3.6 requires it.
  The `list_id` is also inserted into the URL path without encoding (`clickup.rs:89-90`).
  Fix: retry once after `Retry-After`, and percent-encode `list_id`.
- **`job_retry` ignores PLAN 3.3** (`src-tauri/src/slack.rs:959-982`). It requeues a
  failed job even when `job_ops` shows writes, and it moves a `pending` (needs-human) job to
  the agent, which bypasses the review queue.
  Fix: refuse when `job_ops` has rows, and allow `pending` only from an explicit "send to
  reviewer" action.
- **Unauthenticated polling exhausts GitHub's limit.** Without a token, each PR costs
  three calls every 3 minutes, so one PR uses GitHub's 60/h limit.
  Fix: skip polling without a token, or back off on 403/429.

### Not covered

- No live PR was polled. No real ClickUp list or GitHub repository was imported; only the
  mock servers were exercised.
- Detached HEAD and missing-path behaviour of `git::state` was not run.
- The `0ac6bd7` bindings and the `e92e9c6` / `d6c3424` UI were read only for the command
  names.
- `cargo test` and `clippy` were not re-run.

## Follow-up fixes (2026-11-17, after the verdict)

**Commits:** `ee8f735`, `dd53333`, `b505dad` and the PR-poll fix in `src-tauri/src/github.rs`.

- GitHub issue links now use `<owner>/<repo>#<number>` as the external id, so two
  repositories cannot collide.
- The PR poller continues after a failing link, reports an aggregate status error, skips
  links on archived tasks, and backs off to every 30 minutes without a token (the public API
  allows 60 calls/hour).
- Imports read up to 10 pages of 100.
- ClickUp honours one `Retry-After` on 429 and percent-encodes the list id in the path.
- `job_retry` refuses a job that already has `job_ops` rows and a `pending` review job.
- **Still open:** the imported task and its link are still two writes (a crash between them
  leaves an unlinked task), and the page cap is silent. Both are queued for the 1.0 pass.
