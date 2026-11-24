# Task stage routing

Status: local implementation complete; CI pending.

- **Agent / model** — HungerG Free.
- **Commit(s)** — `d93eb42` for the implementation. This ledger follows in a second commit.
- **Files** — `crates/relay-core/src/task_lifecycle.rs`, `src-tauri/src/lifecycle.rs`, `src-tauri/src/{commands,dispatch,grilling,lib,opencode}.rs`, this ledger.
- **What changed** — Core now selects the next Task stage from committed facts. One Tauri runtime module executes that choice. Composer saves, project assignment, Wait approval and Grill closing approval no longer call stage coordinators directly.

## Commands + results

| Command | Result |
| --- | --- |
| `cargo test -p relay-core task_lifecycle -- --nocapture` | First compile exposed a missing Tauri `Manager` import in the runtime module. Fixed it. Final targeted run passed, 7 tests. |
| `cargo test -p relay-core` | Pass, 181 passed, 2 ignored. |
| `cargo check --workspace` | Pass. |
| `pnpm typecheck`, `pnpm test`, `pnpm build` | Pass on Node 24.4.1. Frontend tests: 44 passed. |
| `git diff --check` | Pass. |

## Known risks

- No live desktop or model run exercised automatic stage progression.
- Project assignment refreshes the stored projection before selecting the next stage.
- The repository-wide format check remains red on pre-existing `keybinds.rs`.

## Review checklist

- [ ] Confirm Composer save starts only the stage selected by core.
- [ ] Confirm project assignment refreshes Wait and Grill projections.
- [ ] Confirm approvals dispatch only after the final gate.
- [ ] Confirm stale model results and terminal Tasks do not restart work.

## Opus verdict
