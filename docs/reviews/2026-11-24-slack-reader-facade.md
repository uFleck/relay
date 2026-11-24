# Slack Reference reader facade and window recovery

Status: local implementation complete; CI and live provider acceptance pending.

- **Agent / model** — HungerG Free.
- **Commit(s)** — `9f2e9fc` for the implementation. This ledger follows in a second commit.
- **Files** — `src-tauri/src/slack/reader.rs`, `src-tauri/src/slack.rs`, `src-tauri/src/live.rs`, `src-tauri/src/recovery.rs`, this ledger.
- **What changed** — Added one Slack reader facade that routes a thread Reference to thread policy and a frozen window Reference to window policy. Live exact reads and background recovery now use the same facade. This fixes the observed path where every Slack recovery row called `resolve_thread`, which rejects `SlackWindow`.

## Commands + results

| Command | Result |
| --- | --- |
| `cargo test -p relay --lib slack::reader -- --nocapture` | First run failed to compile because the facade treated a plain resolution as `Option`. Fixed the match arms. Second run passed, 2 tests. |
| `cargo fmt --all -- --check` | Fail on pre-existing `src-tauri/src/keybinds.rs` formatting at `origin/master`. The file was not changed by this package. |
| `git diff --check` | Pass. |

The two tests assert that thread and window Reference keys route to their matching policy.

## Known risks

- No live Slack connector or desktop run exercised window recovery.
- Thread and window modules still own their persistence mechanics. The facade centralizes routing now; a later slice can concentrate the shared commit path.
- The repository-wide format check is red on `keybinds.rs` before this change.

## Review checklist

- [ ] Confirm `SlackWindow` recovery reaches `window::resolve` and `window::persist`.
- [ ] Confirm non-Slack References still use GitHub or ClickUp exact reads.
- [ ] Confirm coalesced reads do not enroll or advance recovery.
- [ ] Confirm no token or raw provider output crosses the facade.

## Opus verdict
