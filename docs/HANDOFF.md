# Relay handoff (for the next agent)

State as of 2026-11-18 02:25 (-03:00). Relay **0.4.0** is released (GitHub release `v0.4.0`); 0.3.2 is what was
installed here and the 0.3.2 → 0.4.0 update has not been run yet. Validated updater path so far: 0.1.0 → … → 0.3.2.

## What works

- **Local task graph**: projects, fixed workflow, tasks, append-only changes, claims with a 30-minute lease,
  links, events, proposals. SQLite + FTS5, migrations through `user_version = 3`.
- **MCP on `127.0.0.1:38547`**: 20 agent tools on `/mcp`, 8 analyst tools on `/mcp/analyst`, separate bearer
  tokens, cross-token requests 401. Agents may submit or reject a proposal; approve is UI-only.
- **Executor**: `relay` is in `C:\Projects\executor-bridges\servers.json`, the bridge proxies 9450 → 38547.
  The full create → claim → requirement → ack → complete → review loop was verified through Docker Executor.
- **Slack capture (P2)**: Socket Mode runtime, 14-day first backfill (shadow), 90-minute episode gap, thread
  replies, edits/deletes, excluded peers, retention, pause. Tokens in Windows Credential Manager. **Ctrl+Alt+S**,
  the tray's **Sync Slack now**, and **Sync now** in Settings run the same pull on demand (user token only; no
  socket needed). The socket catch-up and a manual check never overlap.
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
- **GitHub auth and Sources (0.4.0)**: stored token, then `gh auth token`, then anonymous; Settings shows the gh login.
  The Sources list (Settings, rail, hover) is built from real Slack/JEV/GitHub/ClickUp status.
- **Claude Slack transport (0.4.0)**: headless `claude -p --model haiku` reads DMs through the claude.ai Slack connector
  every 10 minutes (first run 20 s after start), on Ctrl+Alt+S, tray and Sync now; transport picker in Settings → Slack.
  The connector sometimes reports `pending` on a headless start (no retry yet). Running the debug exe uses the real
  `relay.db` and fires this schedule 20 s after launch.
- **UI**: main window (Today, Inbox/transcripts, Projects, Agents, Review, Settings), hover entry, Slack/JEV
  UI, GitHub/git context, import panels, MCP setup with masked tokens.

## Verified

- `cargo test -p relay-core`: **88 passed**.
- `cargo test -p relay --lib`: **26 passed** (1 ignored real smoke).
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`: clean.
- `pnpm typecheck`, `pnpm vitest run` (14), `pnpm build`: pass.
- Settings in the real debug exe (0.4.0 build): Slack transport picker and Claude panel, GitHub "Using your gh session" and Sources rows render. Screenshots were not committed.
- **Not verified**: a clean Claude-transport run after the first (two later runs hit connector `pending`).
- JEV key persisted in Credential Manager across restarts; the installed app runs with a clean DB
  (0 tasks, 0 episodes, 0 jobs).
- Live PR sync: a linked `tauri-apps/tauri#16356` updated to `open` with `checks failing`, and
  `project_git_state` returned `master` / `7db841d` / dirty.
- Updates: 0.1.0 → 0.1.1, 0.1.1 → 0.2.0, 0.2.0 → 0.3.0, 0.3.0 → 0.3.1 and 0.3.1 → 0.3.2 all downloaded,
  verified the signature, restarted and reported the new version.
- In the installed 0.3.2, `Ctrl+Alt+S` from `/` opened `/?view=settings`.
- On-demand sync: from a hidden debug app with no token, `Ctrl+Alt+S` opened Settings (`/?view=settings`), and
  `slack_sync_now` returned `Slack is not configured. Paste your user token in Settings → Slack.`

## Not verified (do these before 1.0)

1. **Real Slack**. The socket has never talked to Slack, and the on-demand pull has not run against it either.
   Create the app with `docs/SLACK_SETUP.md`, paste the `xoxp-` and `xapp-` tokens, confirm `auth.test`, the
   14-day backfill, a real DM, thread replies and JEV. `Ctrl+Alt+S` exercises the same loop without Socket Mode.
   Workspace approval may be required; a free workspace you own can validate the code.
2. **Real ClickUp import**. Only mock servers cover the parser; no ClickUp list was imported.
3. **Atomicity gaps** flagged by the Opus review: an analyst write and its `job_ops` row are two
   transactions, and an imported task and its link are two writes. A crash between them can duplicate one
   record on retry. Close these before 1.0.
4. **Manual Windows checks**: expanded hover, drag/snap, 150% scaling, title-bar actions, click-through.
5. **Imports** cap at 10 pages with no "truncated" warning; a bigger list imports partially.
6. **Reviews**: Opus found 8 blocking items and all are fixed except the atomicity above; its "should
   fix" list is in the two records.

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
| `2e2da6c` | gh credentials, real Sources, Claude Slack transport |
| `abb7971` | Release 0.4.0 |
| `542716f` | ClickUp/Issues import |
| `220c74e` | repoPath + PR metadata in the snapshot |
| `d163fe1` | Job requeue + next queued job |
| `9064ec9` | On-demand MCP tokens |
| `163d5f6` | Import pagination (10 pages) |
| `135a070` | Release 0.3.0 |
| `ee8f735` / `dd53333` / `b505dad` | Opus review fixes: peers, isolation, job guards, retention |
| `22e0359` | Catch-up pagination, rescreen at close, deletions |
| `53d84eb` | Release 0.3.1 |
| `ed38883` | On-demand Slack sync (`Ctrl+Alt+S`, tray, Settings) |
| `adbadb7` | Release 0.3.2 |

Everything is on `origin/master` except work committed after the last push; push before handing off.

## Next for 1.0

1. Paste Slack tokens; fix whatever the real socket/backfill reveals.
2. Run one real ClickUp import; confirm the task list and the import counters.
3. Make the analyst write + `job_ops` atomic in the store, and the import task + link atomic; add the
   `truncated` flag for the page cap.
4. Manual hover/scaling checks and the first-run/onboarding copy.
5. Cut **1.0.0** only after 1-4 pass.

## Rules

- Windows first; no native dialogs except the folder picker; secrets never reach git and are only revealed on
  demand. The signing key is `%USERPROFILE%\.relay\signing.key`; never print it or a third-party token.
- DeepSeek work is `[deepseek]`-prefixed and ledgered in `docs/reviews/`; Opus writes the verdict.
- **Never run destructive git commands** (`reset --hard`, `checkout -- .`, `clean -fd`, `commit --amend`,
  `rebase`) while the tree has uncommitted work. Stage explicit paths, never `git add -A` while another agent
  works, and make a new commit.
- Do not push a release tag before `release.ps1` has its version fields and secrets in place.
