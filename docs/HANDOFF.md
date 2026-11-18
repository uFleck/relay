# Relay handoff (for the next agent)

State as of 2026-11-17 23:30 (-03:00). Relay 0.2.0 is installed on this machine and updates itself from
`uFleck/relay`; 0.3.0 (git/PR + imports) is committed and waiting for its release.

## What works

- **Local task graph**: projects, fixed workflow, tasks, append-only changes, claims with a 30-minute lease,
  links, events, proposals. SQLite + FTS5, migrations through `user_version = 3`.
- **MCP on `127.0.0.1:38547`**: 20 agent tools on `/mcp`, 8 analyst tools on `/mcp/analyst`, separate bearer
  tokens, cross-token requests 401. Agents may submit or reject a proposal; approve is UI-only.
- **Executor**: `relay` is in `C:\Projects\executor-bridges\servers.json`, the bridge proxies 9450 → 38547.
  The full create → claim → requirement → ack → complete → review loop was verified through Docker Executor.
- **Slack capture (P2)**: Socket Mode runtime, 14-day first backfill (shadow), 90-minute episode gap, thread
  replies, edits/deletes, excluded peers, retention, pause. Tokens in Windows Credential Manager.
- **JEV (P3)**: real two-stage calls (screen + classify), routing in code, jobs with one live watermark per
  conversation, job-scoped analyst writes with `<jobId>:<opIndex>` idempotency.
- **Headless reviewer**: one `opencode run --agent relay-analyst` at a time, isolated config, 180 s timeout,
  Win32 Job Object kill, logs in `%APPDATA%\dev.relay.app\analyst\`. A job that exits without `jobs_complete`
  is marked failed; a failed job can be retried and the next queued job starts automatically.
- **Live loop evidence**: a simulated DM pair routed `new_task` (`is_new_task` 0.89, `complete` 0.90) and the
  reviewer created "Verify the RC build" in Inbox/Todo. Test data was cleared afterwards.
- **Git and PRs (P4)**: project `repo_path`, branch/HEAD/dirty via `git`, PR links polled every 3 minutes
  (state, draft/merged, checks, review), PR metadata in the snapshot.
- **Imports (P5)**: ClickUp list import and GitHub Issues import, one-shot and idempotent by link; nothing is
  pushed back. Tokens in Credential Manager; ClickUp uses the raw token header.
- **Release**: public repo, `Production` secrets, `scripts/release.ps1`. Measured 128 s / 145 s / 180 s total
  for 0.1.0 / 0.1.1 / 0.2.0. The updater was validated twice on a real install.
- **UI**: main window (Today, Inbox/transcripts, Projects, Agents, Review, Settings), hover entry, Slack/JEV
  UI, GitHub/git context, import panels, MCP setup with masked tokens.

## Verified

- `cargo test -p relay-core`: **69 passed**.
- `cargo test -p relay --lib`: **22 passed**.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`: clean.
- `pnpm typecheck`, `pnpm vitest run` (14), `pnpm build`: pass.
- JEV key persisted in Credential Manager across restarts; the installed 0.2.0 runs with a clean DB
  (0 tasks, 0 episodes, 0 jobs).
- `v0.2.0` was offered to an installed 0.1.1, downloaded, signature-verified, installed and reported 0.2.0.

## Not verified (do these before 1.0)

1. **Real Slack**. The socket has never talked to Slack. Create the app with `docs/SLACK_SETUP.md`, paste the
   `xoxp-` and `xapp-` tokens, confirm `auth.test`, the 14-day backfill, a real DM, thread replies and JEV.
   Workspace approval may be required.
2. **Real imports and PR polling**. Only mock servers cover the parsers; no ClickUp list, GitHub issue or PR
   was imported/polled live.
3. **Manual Windows checks**: expanded hover, drag/snap, 150% scaling, title-bar actions, click-through.
4. **Opus review** of the DeepSeek P2-P5 packages in `docs/reviews/` (the verdicts are being written).
5. **Failed-job retry** and the token Reveal/Copy paths were not exercised in the desktop app.

## Commits (recent)

| Commit | What |
| --- | --- |
| `e6d7064` | Release 0.2.0 |
| `df5f34a` | Slack capture + jobs store |
| `17b1d57` | Slack runtime + job-scoped analyst |
| `8016700` | Headless reviewer (also swept up early UI files) |
| `f5ca859` | Created-task scope + rescreen |
| `c5a4fc6` | git/GitHub clients |
| `fff648c` | PR sync + project git state |
| `542716f` | ClickUp/Issues import |
| `220c74e` | repoPath + PR metadata in the snapshot |
| `d163fe1` | Job requeue + next queued job |
| `9064ec9` | On-demand MCP tokens |
| `163d5f6` | Import pagination (10 pages) |

Everything is on `origin/master` except work committed after the last push; push before handing off.

## Next for 1.0

1. Paste Slack tokens; fix whatever the real socket/backfill reveals.
2. Run one real ClickUp import and one real PR poll; confirm the task sheet context.
3. Cut **0.3.0** (git/PR + imports), then **1.0.0** once the live checks pass and the Opus review is in.
4. Manual hover/scaling checks and the first-run/onboarding copy.

## Rules

- Windows first; no native dialogs except the folder picker; secrets never reach git and are only revealed on
  demand. The signing key is `%USERPROFILE%\.relay\signing.key`; never print it or a third-party token.
- DeepSeek work is `[deepseek]`-prefixed and ledgered in `docs/reviews/`; Opus writes the verdict.
- **Never run destructive git commands** (`reset --hard`, `checkout -- .`, `clean -fd`, `commit --amend`,
  `rebase`) while the tree has uncommitted work. Stage explicit paths, never `git add -A` while another agent
  works, and make a new commit.
- Do not push a release tag before `release.ps1` has its version fields and secrets in place.
