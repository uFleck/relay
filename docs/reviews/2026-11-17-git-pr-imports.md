# Git/PR context, imports and runner hardening

**Status:** backend complete; the two UI slices are `e92e9c6` (git context) and a pending
ClickUp/Issues import UI.
**Agent / model:** DeepSeek V4.1.
**Commits:** `fff648c` (PR sync + git state), `542716f` (ClickUp/Issues import), `220c74e`
(repo path + PR metadata in the snapshot), `d19bf0f` (link lookup test), `d163fe1`
(job requeue + next-queued job), `9064ec9` (on-demand MCP tokens), `0ac6bd7` (import
bindings).

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

- **No live PR was polled** and no real ClickUp list or GitHub issue was imported; only
  mock servers cover the parsers.
- One page per import (100 items); a bigger list needs pagination before 1.0 is complete.
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
