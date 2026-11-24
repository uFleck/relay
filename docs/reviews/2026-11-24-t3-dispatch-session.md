# T3 dispatch session adapter

Status: local implementation complete; CI pending.

- **Agent / model** — HungerG Free.
- **Commit(s)** — `eaee295` for the implementation. This ledger follows in a second commit.
- **Files** — `src-tauri/src/dispatch.rs`, `crates/relay-core/src/t3_session.rs`, this ledger.
- **What changed** — Routed Tauri dispatch through the core thread session. The adapter now only maps CLI results, runs blocking work, creates the progress row and publishes events. Removed duplicate hydration and reconciliation sequencing.

## Commands + results

| Command | Result |
| --- | --- |
| `cargo test -p relay-core t3_session -- --nocapture` | Pass, 2 tests. |
| `cargo test -p relay --lib t3_cli::tests -- --nocapture` | Pass, 7 passed, 1 ignored real-CLI smoke. |
| `cargo test -p relay-core` | Pass, 183 passed, 2 ignored. |
| `cargo check --workspace` | Pass. |
| `pnpm typecheck`, `pnpm test`, `pnpm build` | Pass on Node 24.4.1. Frontend tests: 44 passed. |
| `git diff --check` | Pass. |

## Known risks

- T3 progress and continuation still use their own adapter sequencing.
- No live T3 server was used.
- The repository-wide format check remains red on pre-existing `keybinds.rs`.

## Review checklist

- [ ] Confirm start, hydration and reconciliation all run through core.
- [ ] Confirm cancellation still allows a late identity to bind.
- [ ] Confirm a bound session creates exactly one progress row.
- [ ] Confirm manual linking keeps its path-membership check.

## Opus verdict
