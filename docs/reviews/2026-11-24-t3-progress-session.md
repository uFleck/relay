# T3 progress session interface

Status: local implementation complete; CI pending.

- **Agent / model** — HungerG Free.
- **Commit(s)** — `940105e` for the implementation. This ledger follows in a second commit.
- **Files** — `crates/relay-core/src/t3_session.rs`, `src-tauri/src/progress.rs`, this ledger.
- **What changed** — Extended the core T3 session with progress reads, captured and current observation commits, send confirmation, draft reopen and explicit closure. The Tauri progress adapter now calls that interface instead of assembling store transitions itself.

## Commands + results

| Command | Result |
| --- | --- |
| `cargo test -p relay-core t3_session -- --nocapture` | Pass, 2 tests. |
| `cargo test -p relay-core t3_progress -- --nocapture` | Pass, 13 tests. |
| `cargo test -p relay --lib progress::tests -- --nocapture` | Pass, 1 test. |
| `cargo test -p relay-core` | Pass, 183 passed, 2 ignored. |
| `cargo check --workspace` | Pass. |
| `pnpm typecheck`, `pnpm test`, `pnpm build` | Pass on Node 24.4.1. Frontend tests: 44 passed. |
| `git diff --check` | Pass. |

## Known risks

- The adapter still sequences CLI send delivery. Core owns the durable transitions around it.
- No live T3 server was used.
- The repository-wide format check remains red on pre-existing `keybinds.rs`.

## Review checklist

- [ ] Confirm captured reads still reject stale revisions.
- [ ] Confirm current reads preserve Returned and Failed metadata behavior.
- [ ] Confirm send confirmation and resend use the session interface.
- [ ] Confirm closure still fences in-flight reads.

## Opus verdict
