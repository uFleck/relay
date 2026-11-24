# Shared Windows child-process module

Status: local implementation complete; CI pending.

- **Agent / model** — HungerG Free.
- **Commit(s)** — `0570ca9` for the implementation. This ledger follows in a second commit.
- **Files** — `src-tauri/src/process.rs`, `src-tauri/src/slack/claude.rs`, `src-tauri/src/lib.rs`, this ledger.
- **What changed** — Added one process module for hidden startup, Job Object ownership, bounded stdout and stderr, UTF-8 stdin, deadlines and deterministic teardown. Slack now uses it with a 1 MiB output cap and whole-tree cleanup.

## Commands + results

| Command | Result |
| --- | --- |
| `cargo test -p relay --lib process::tests -- --nocapture` | First compile exposed two closure typing errors. Fixed both. A suspended-start fixture then failed because Windows could not open the suspended initial thread. Removed suspended startup and kept the proven hidden-spawn plus immediate Job attach path. Final run passed, 4 tests. |
| `cargo test -p relay --lib slack::reader -- --nocapture` | Pass, 2 tests. |
| `cargo check --workspace` | Pass. |
| `cargo test -p relay-core` | Pass, 174 passed, 2 ignored. |
| `pnpm typecheck`, `pnpm test`, `pnpm build` | Pass on Node 24.4.1. Frontend tests: 44 passed. |
| `cargo fmt --all -- --check` | Fail on pre-existing `src-tauri/src/keybinds.rs` formatting. This package does not touch that file. |
| `git diff --check` | Pass. |

The real-process tests cover bounded reads, hidden startup, timeout cleanup and a grandchild that inherits the output pipes.

## Known risks

- No live Claude or Slack connector run exercised the new runner.
- A descendant spawned before Job Object assignment is not covered. The tested launcher tree inherits the Job.
- Suspended startup was rejected after a real Windows failure. The shared module does not attempt it.
- The repository-wide format check remains red on `keybinds.rs`.

## Review checklist

- [ ] Confirm Job Object termination happens before pipe-reader joins.
- [ ] Confirm output caps keep draining after the cap.
- [ ] Confirm timeout and grandchild fixtures leave no child process behind.
- [ ] Confirm Slack still maps timeout to `ClaudeError::Timeout`.

## Opus verdict
