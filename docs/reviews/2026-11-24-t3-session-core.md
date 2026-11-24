# Core T3 thread session

Status: local implementation complete; CI pending.

- **Agent / model** — HungerG Free.
- **Commit(s)** — `cc76b1f` for the implementation. This ledger follows in a second commit.
- **Files** — `crates/relay-core/src/t3_session.rs`, `crates/relay-core/src/lib.rs`, this ledger.
- **What changed** — Added one core T3 session interface with an injected transport. Core now owns the durable start, partial-identity hydration, binding and read-only reconciliation sequence. Fake transports exercise complete and lost-response starts.

## Commands + results

| Command | Result |
| --- | --- |
| `cargo test -p relay-core t3_session -- --nocapture` | Pass, 2 tests. |
| `cargo test -p relay-core` | Pass, 183 passed, 2 ignored. |
| `cargo check --workspace` | Pass. |
| `pnpm typecheck`, `pnpm test`, `pnpm build` | Pass on Node 24.4.1. Frontend tests: 44 passed. |
| `git diff --check` | Pass. |

## Known risks

- The Tauri dispatch adapter still uses the older direct sequence. A follow-up slice will migrate it.
- No live T3 server was used.
- The repository-wide format check remains red on pre-existing `keybinds.rs`.

## Review checklist

- [ ] Confirm a complete identity binds in one durable transition.
- [ ] Confirm a partial identity is saved before hydration.
- [ ] Confirm reconciliation requires one exact title and a matching profile.
- [ ] Confirm transport errors keep uncertain starts fenced.

## Opus verdict
