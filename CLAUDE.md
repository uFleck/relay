# Relay

## Rules

- **Windows first.** Test and ship on Windows before anything else.
- **Local SQLite is the source of truth.** ClickUp and GitHub Issues are link-level projections, never a full mirror. Read `docs/PLAN.md` before adding a table or a tool.
- **Two surfaces:** the main window comes from `src-tauri/tauri.conf.json`; the hover window is built hidden in `src-tauri/src/hover.rs` (Spawner's `companion.rs` pattern). The main UI is in progress under `src/`; the hover entry (`hover.html`) is still missing. Reconcile `src/ipc.ts` with the Rust commands before changing either side.
- **No native dialogs except the folder picker.** No `ask`, `message`, `confirm` or `alert` from `@tauri-apps/plugin-dialog` or the browser. Use an in-app dialog. `dialog:allow-open` is the only dialog permission granted.
- **Secrets never reach the frontend and never go in git.** Slack/JEV/GitHub/ClickUp tokens belong in Windows Credential Manager later; the UI sees status and last 4 characters only. Updater keys live outside the repo.
- **Policy lives in `relay-core` services, not in adapters.** `src-tauri` IPC commands and MCP tools stay thin. `relay-core` never depends on Tauri.
- **Fast loop:** `cargo test -p relay-core` does not compile Tauri.
- **Ids and time:** ULIDs as `String`, RFC 3339 UTC timestamps as `String`, `serde(rename_all = "camelCase")` on IPC/model structs; Rust stays `snake_case`.
- **DB:** WAL for file databases, foreign keys ON, migrations only through `PRAGMA user_version` (`crates/relay-core/migrations/`). FTS5 indexes task title/notes.
- **Use pnpm.** Node 24.4.1 (`.node-version`).
- **Do not push unasked.** Commits stay local until the user asks to push.
- **Never rewrite history or discard uncommitted work.** No `git reset --hard`, `git checkout -- .`, `git clean -fd`, `git commit --amend`, or `git rebase` while another agent may have uncommitted files. Stage explicit paths and make a new commit. If history needs repair, use a separate worktree or ask the user.
- **Review ledger:** every work package gets `docs/reviews/YYYY-MM-DD-<slug>.md` (agent/model, commit, files, commands/results, risks, checklist, empty "Opus verdict"); DeepSeek commits use a `[deepseek]` subject prefix.
- **Updater/release:** `createUpdaterArtifacts` is on and `plugins.updater.pubkey` is set from the locally generated pair. Private key + password live outside the repo at `%USERPROFILE%\.relay\signing.key` and `.password`; never print or commit them. `scripts/release.ps1` is the local fast path (warm target, target < 5 min); `.github/actions/release` holds the shared CI steps. `scripts/version.ps1` bumps all five version fields and refreshes `Cargo.lock` offline. `release.ps1` refuses a dirty tree, a version-field mismatch, a missing or unpushed tag, an existing GitHub release and an installer without the exact version; it warns at 300s and fails at 420s after the build, before upload (`-ForceOverBudget` overrides). Versions with a prerelease suffix publish with `--prerelease`, so `/releases/latest` stays on stable. Add the two values to the `Production` GitHub environment before the first CI release.
