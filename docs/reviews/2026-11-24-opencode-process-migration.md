# OpenCode shared process migration

Status: local implementation complete; CI pending.

- **Agent / model** — HungerG Free.
- **Commit(s)** — `a4561b1` for the implementation. This ledger follows in a second commit.
- **Files** — `src-tauri/src/opencode.rs`, `src-tauri/src/process.rs`, this ledger.
- **What changed** — Routed isolated OpenCode attempts through the shared process module. Removed the duplicate Job Object, bounded reader, deadline loop and unreliable suspended-start path. Kept private config, environment isolation, canary detection and sanitized failures in the OpenCode adapter.

## Commands + results

| Command | Result |
| --- | --- |
| `cargo test -p relay --lib opencode::tests -- --nocapture` | Pass, 9 tests. |
| `cargo test -p relay --lib process::tests -- --nocapture` | Pass, 5 tests, including stdin failure, timeout and grandchild cleanup. |
| `cargo check --workspace` | Pass. |
| `cargo test -p relay-core` | Pass, 174 passed, 2 ignored. |
| `pnpm typecheck`, `pnpm test`, `pnpm build` | Pass on Node 24.4.1. Frontend tests: 44 passed. |
| `git diff --check` | Pass. |

## Known risks

- No real OpenCode model call exercised the migrated runner.
- The shared runner uses hidden spawn plus immediate Job attach. It does not suspend the child.
- A broken stdin pipe no longer hides a classified nonzero exit.
- The repository-wide format check remains red on pre-existing `keybinds.rs`.

## Review checklist

- [ ] Confirm private config and child environment construction are unchanged.
- [ ] Confirm canary detection still runs on successful output.
- [ ] Confirm nonzero exits take precedence over stdin errors.
- [ ] Confirm timeouts and grandchild cleanup come from the shared module.

## Opus verdict
