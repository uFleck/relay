# Relay handoff (for Opus)

State as of 2026-11-17 21:15 (-03:00). Read this before touching the UI, MCP or the window runtime.

## Commits

| Commit | Owner | What |
| --- | --- | --- |
| `924aae1` | DeepSeek | Scaffold: Tauri shell, core model/store, release pipeline |
| `1e26ee3` | Opus | Hover window, tray, hotkey, updater store |
| `c4721f9` | DeepSeek | Core store, claims, changes, proposals |
| `70f8eae` | DeepSeek | Review ledger + release audit (fixed `--verify-tag`) |
| `91e4f0a` | DeepSeek | UI snapshot + IPC commands |
| `6b2bad9` | DeepSeek | JEV System One client + routing (16 tests) |
| `70be4a4` | DeepSeek | Core regression tests, FTS escaping, claim busy timeout |
| `9b9492d` | DeepSeek | Release hardening (five version fields, prerelease, budget gate) |
| `c2a5df3` | Opus | Main window UI + hover entry |
| `3eccbeb` | Opus | Agent MCP endpoint + analyst boundary |
| `00bdacf` | maintenance | Agents may submit/reject proposals; approve is UI-only |

Working tree: clean as of this note. The MCP settings sheet is committed (`3e3042c`).

## Verified

- `cargo test -p relay-core`: **45 passed**.
- `cargo test -p relay --lib`: **21 passed** (MCP 18 + hover 3).
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`: clean.
- `pnpm typecheck`, `pnpm test` (10), `pnpm build` (index + hover.html): pass.
- MCP end-to-end on the running app: initialize `relay`/`2025-06-18`, **20 agent tools**, `tasks_list` empty, agent token on `/mcp/analyst` 401, analyst token on `/mcp` 401, **8 analyst tools**.
- **Full AI loop through Executor:** create → claim as T3 Code → Slack requirement → `progress` returned 1 unacked → ack → 0 → complete → **In Review** → archived. The main window showed the review task.
- Settings → Agents & MCP live: green running state, URLs, tokens, Copy, Executor/OpenCode snippets.
- Main UI rendered with the real snapshot; empty state is honest. Hover tab (52×64) renders and pinning makes it visible.
- Final signed build with full UI + MCP: **176.5s (2m56s)**, NSIS **4.57 MiB** + `.sig`; the earlier cold build was 268.3s.

## MCP

- Agent: `http://127.0.0.1:38547/mcp`, 20 tools, token `%APPDATA%\dev.relay.app\mcp-agent.token`.
- Analyst: `http://127.0.0.1:38547/mcp/analyst`, 8 tools (job tools stubbed), token `mcp-analyst.token`.
- `invoke("mcp_status")` returns `{ running, port, url, analystUrl, error, agentToken, analystToken }`; `mcp://status` emits the same.
- Agent `proposals_decide` is **reject-only**; a human approves in the UI.
- Port 38547 is fixed and fails loudly; there is no fallback port.
- **Executor is wired**: `C:\Projects\executor-bridges\servers.json` has a `relay` entry (agent URL + token), the bridge proxies `9450 -> 38547`, and `node sync.mjs` deployed the `relay` app. `executor call tools.relay.projects_list` returns the real Inbox; `tools.relay.tasks_list` returns `{"tasks":[]}`. The bridge (Spawner service) and Relay must both be running.
- Settings → Agents & MCP shows the green status, URLs, tokens, Copy buttons, and the Executor/OpenCode snippets (`3e3042c`).

## Runtime commands

`hover_ready`, `hover_place(x,y,width,height)` in physical pixels, `hover_drag`, `hover_pointer`, `hover_focus`, `hover_set_pinned`, `hover_state`, `set_paused`, `set_dnd`, `check_for_updates`, `show_main_window` (no args), `open_main(route)`, `relay_snapshot`, `ack_change({taskId,changeId,rev})`, `decide_proposal({proposalId,decision,rev})`, `mcp_status`.

Events: `relay://state`, `relay://open` (route string), `relay://change`, `update://check`, `mcp://status`.

## UI status

Implemented and committed in `c2a5df3`: Today, task sheet with change timeline, Inbox/Projects/Agents/Review/Settings views, review queue, agent strip, hover entry, updater wiring, route handling, hover placement/snap, pulse, DND/pin.

Manual checks still needed on Windows:

1. Hover expand/collapse at 100% and 150% scaling, and on a second monitor; the expanded panel could not be triggered while a game held the cursor.
2. Title bar drag/double-click/minimize/maximize/close; close hover color.
3. Drag/snap across monitors and left-edge mirroring.
4. Hover buttons work on the first click without taking focus.
5. MCP settings sheet is live and verified (green state, URLs, token copy, snippets); no manual check left there.

## Release (live)

- Repo: <https://github.com/uFleck/relay>, **public** (the updater downloads assets without a token).
- `Production` environment with `TAURI_SIGNING_PRIVATE_KEY` and `_PASSWORD`; key at `%USERPROFILE%\.relay\signing.key`.
- Cut releases locally: `pwsh scripts/release.ps1 -Version X.Y.Z -SkipInstall` after `version.ps1`, the bump commit and the tag.
- Measured: v0.1.0 **128.4 s**, v0.1.1 **145.4 s** total incl. upload (build gate 123.8 s / 140.9 s).
- Updater validated on a real install: 0.1.0 → detected 0.1.1 → signature verified → restart → 0.1.1.
- The CI workflow is **dispatch-only** (no `relay-release` runner exists); a tag push must not queue a job.

## Next packages

1. **Slack Socket Mode** (P2): app/user tokens, DM events, episode segmentation, catch-up polling, fixtures/tests. No UI until capture works.
2. **JEV jobs + analyst runner** (P3): `analysis_jobs` store, job-scoped analyst writes, `job_ops` idempotency, OpenCode headless with an isolated config and Job Object kill; then connect the JEV client to Slack episodes.
3. **ClickUp/GitHub projection** (P5): link/import/pull status only; no title/description push.
4. **Release**: rebuild signed installer after MCP settings, measure, create `uFleck/relay` (or the real owner/name), add the `Production` signing secrets, then test tag → `latest.json` → update.

## Rules

- Windows first; no native dialogs except the folder picker; secrets never reach the frontend or git.
- DeepSeek work is `[deepseek]`-prefixed and ledgered in `docs/reviews/`; Opus writes the verdict.
- **Never run destructive git commands** (`reset --hard`, `checkout -- .`, `clean -fd`, `commit --amend`, `rebase`) while the tree has uncommitted work. Stage explicit paths and make a new commit.
- Do not push; do not print the signing key or third-party tokens.
- The real DB is clean (0 tasks). Demo seeding, when needed, uses `%TEMP%\opencode\seed_relay_demo.py` and is removed afterwards.
