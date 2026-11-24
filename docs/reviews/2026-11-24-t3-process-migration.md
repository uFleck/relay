# T3 CLI shared process migration

Status: local implementation complete; CI pending.

- **Agent / model** — HungerG Free.
- **Commit(s)** — `c976ef7` for the implementation. This ledger follows in a second commit.
- **Files** — `src-tauri/src/t3_cli.rs`, `src-tauri/src/process.rs`, this ledger.
- **What changed** — Routed T3 start, show, list and send through the shared process module. Removed the duplicate Job Object, bounded reader and deadline loop. Preserved T3's behavior of ignoring a broken stdin pipe when the CLI already exited with a classified error.

## Commands + results

| Command | Result |
| --- | --- |
| `cargo test -p relay --lib t3_cli::tests -- --nocapture` | Pass, 7 passed, 1 ignored real-CLI smoke. The argv and UTF-8 stdin fixture passes through the shared runner. |
| `cargo test -p relay --lib process::tests -- --nocapture` | Pass, 4 tests. |
| `cargo check -p relay` | Pass. |
| `cargo check --workspace` | Pass. |
| `cargo test -p relay-core` | Pass, 174 passed, 2 ignored. |
| `pnpm typecheck`, `pnpm test`, `pnpm build` | Pass on Node 24.4.1. Frontend tests: 44 passed. |
| `git diff --check` | Pass. |

## Known risks

- The real T3 lost-start smoke remains ignored and was not run.
- No live T3 server was used.
- The shared runner maps Job assignment failure to `CliError::LauncherMissing`, matching the previous T3 behavior.
- The repository-wide format check remains red on pre-existing `keybinds.rs`.

## Review checklist

- [ ] Confirm T3 argument construction and UTF-8 stdin are unchanged.
- [ ] Confirm nonzero exits still reach `classify_start` and `classify_send`.
- [ ] Confirm a broken stdin pipe does not hide a classified CLI error.
- [ ] Confirm timeout still maps to `CliError::Timeout`.

## Opus verdict
