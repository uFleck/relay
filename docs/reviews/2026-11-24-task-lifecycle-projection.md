# Task lifecycle projection

Status: local implementation complete; CI pending.

- **Agent / model** — HungerG Free.
- **Commit(s)** — `8f58ac0` for the implementation. This ledger follows in a second commit.
- **Files** — `crates/relay-core/src/task_lifecycle.rs`, `crates/relay-core/src/lib.rs`, `crates/relay-core/src/t3_progress.rs`, `crates/relay-core/src/v2/{briefing,grilling,store}.rs`, this ledger.
- **What changed** — Added one core Task projection for Briefing, Grilling and T3 progress. The store now mirrors that projection instead of embedding stage policy. Briefing and Grilling delegate status decisions to the same module.

## Commands + results

| Command | Result |
| --- | --- |
| `cargo test -p relay-core task_lifecycle -- --nocapture` | First compile exposed a type alias derive, a stale local reference and private model imports. Fixed all three. Final targeted run passed, 4 tests. |
| `cargo test -p relay-core` | Pass, 178 passed, 2 ignored. |
| `cargo check --workspace` | Pass. |
| `pnpm typecheck`, `pnpm test`, `pnpm build` | Pass on Node 24.4.1. Frontend tests: 44 passed. |
| `git diff --check` | Pass. |

## Known risks

- Runtime stage triggers still call Briefing, Grilling and Dispatch directly. A follow-up slice will route them through one next-stage decision.
- No live desktop or model run exercised the projection.
- The repository-wide format check remains red on pre-existing `keybinds.rs`.

## Review checklist

- [ ] Confirm Brief, Grilling and T3 progress produce the same public projection shape.
- [ ] Confirm Wait approval and Grill closing approval remain distinct.
- [ ] Confirm failure stage and reason survive projection.
- [ ] Confirm store writes no longer embed stage policy.

## Opus verdict
