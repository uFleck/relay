# Isolated OpenCode attempt interface

Status: local implementation complete; CI pending.

- **Agent / model** — HungerG Free.
- **Commit(s)** — `daadef3` for the implementation. This ledger follows in a second commit.
- **Files** — `src-tauri/src/opencode/attempt.rs`, `src-tauri/src/opencode.rs`, `src-tauri/src/grilling.rs`, this ledger.
- **What changed** — Added one isolated-attempt interface that resolves Settings, creates the private scratch tree, proves the CLI boundary and runs the child. Briefing and Grilling now supply typed input and interpret their own validated result.

## Commands + results

| Command | Result |
| --- | --- |
| `cargo test -p relay --lib opencode::tests -- --nocapture` | Pass, 9 tests. |
| `cargo test -p relay --lib grilling::tests -- --nocapture` | Pass, 1 test. |
| `cargo check --workspace` | Pass. |
| `cargo test -p relay-core` | Pass, 174 passed, 2 ignored. |
| `pnpm typecheck`, `pnpm test`, `pnpm build` | Pass on Node 24.4.1. Frontend tests: 44 passed. |
| `git diff --check` | Pass. |

## Known risks

- No real model call exercised the new interface.
- The low-level isolation implementation remains in the parent OpenCode module. The new interface owns the sequence and hides it from both workflows.
- The one-tool Grilling endpoint and its permission model are unchanged.
- The repository-wide format check remains red on pre-existing `keybinds.rs`.

## Review checklist

- [ ] Confirm Briefing and Grilling no longer construct scratch paths or low-level invocations.
- [ ] Confirm both paths use the same boundary proof and sanitized failure prefix.
- [ ] Confirm Grilling still injects exactly one bearer and one Reference tool.
- [ ] Confirm no credential or bearer reaches the frontend or logs.

## Opus verdict
