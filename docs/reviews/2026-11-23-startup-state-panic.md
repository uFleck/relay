# Startup panic after the UI parity release

Status: fix implemented, v0.10.1 pending publish.

- **Agent / model** - OpenCode session on `opencode-go/hy4-flash-free`, the only model used in this run. Worktree `C:\Users\Pichau\.t3\worktrees\relay\relay-v2-startup-fix` on `relay-v2/startup-fix`, branched from `master` (`7d41c1e`). No subagent was used for the fix. The stale main checkout was not touched.

- **Symptom** - The operator reported that installed 0.10.0 opens then closes. A launch loop on the installed exe showed a window in 10 of 10 runs because the harness ended the process as soon as the window appeared. Watching a full startup showed the process exiting with a panic in stderr.

- **Root cause** - `src-tauri/src/lib.rs` managed `progress::ProgressState::default()` as a plain value, while `progress::init` and every progress command ask for `Arc<ProgressState>`. Tauri keys managed state by the requested type, so `app.state::<Arc<ProgressState>>()` panicked at `tauri-2.12.6/src/lib.rs:761` during setup. This is a pre-existing mismatch that the 0.10.0 build exposed on a fresh start. The exact panic was: `state() called before manage() for alloc::sync::Arc<relay_lib::progress::ProgressState>`.

- **Files** - `src-tauri/src/lib.rs` now manages `Arc::new(progress::ProgressState::default())`; `scripts/smoke-start.ps1` (new) launches a build, waits for a real window and asserts the process stays alive afterwards, capturing stderr on failure; this ledger added.

- **Feedback loop** - `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/smoke-start.ps1 -Exe <exe> -WaitSeconds 10 -AliveSeconds 10`. Red against the installed 0.10.0 exe: exit 1 with the state panic in stderr. Green against the rebuilt exe: `PASS: window appeared and process stayed alive for 10s`. The check reaches the real setup path and asserts the user's exact symptom, so it can go red on this bug.

- **Commands + results** (Windows) - `cargo check -p relay` clean. `pnpm tauri build` with signing env and the warm shared target produced a signed installer and `.sig`. `cargo test -p relay-core` was re-run at the release bump: 174 passed, 0 failed, 2 ignored. `pnpm typecheck` clean and `pnpm test` 44 passed.

- **Known risks / exact gaps** - The smoke check needs a GUI session and was run on this Windows desktop; it is not a headless CI test. If another Relay instance is already running, the single-instance plugin may hand off and make the new process exit, so the script is for a manual check with no other instance running. No test covers the Tauri state registry directly; a Rust unit test cannot exercise `app.state()` without building the app. The installer itself was not run as part of the smoke check.

- **Review checklist** - (1) Same state type is managed and read. (2) Red/green demonstrated on real exes. (3) Signed build succeeds. (4) Smoke script committed so the check can be repeated.

- **Opus verdict** - (left empty; only hy4-flash-free was used in this run)
