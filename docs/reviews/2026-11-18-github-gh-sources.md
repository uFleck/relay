# GitHub `gh` credentials and real Sources status

**Status:** final
**Agent / model:** Claude Sonnet (implementation); orchestrated and checked by Claude Sonnet 5.5
**Commit(s):** `2e2da6c` (shared with the other 2026-11-18 ledger; the two packages touch `slack.rs`, `ipc.ts` and `lib.rs` in the same hunks, so they were not split)
**Files:** `src-tauri/src/github.rs`, `src-tauri/src/imports.rs`, `src-tauri/src/commands.rs`, `crates/relay-core/src/view.rs`,
`src/app/Github.tsx`, `src/app/Rail.tsx`, `src/app/Views.tsx`, `src/hover/HoverApp.tsx`, `src/App.tsx`, `src/types.ts`, `src/ipc.ts`

## What changed

- GitHub credentials resolve in this order: stored token (Credential Manager), then `gh auth token`, then anonymous.
  `gh` runs without a console window, with a 5 s timeout; its output is never logged. The `gh` token and login are cached
  and dropped on a 401. The issue import uses the same resolver (`github::current_token`).
- Settings → GitHub shows "Using your gh session (login)" and the token field becomes an optional override.
- The snapshot's Sources list (Settings, rail dots, hover) is built from real Slack, JEV, GitHub and ClickUp status
  instead of an always-empty `Vec`. `SyncState::Off` is the neutral state (no token, no error).

## Commands + results (2026-11-18, before the release commit)

| Command | Result |
| --- | --- |
| `cargo test -p relay-core` | 88 passed |
| `cargo test -p relay --lib` | 26 passed, 1 ignored (`real_smoke`) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo fmt --all -- --check` | clean |
| `pnpm typecheck` | pass |
| `pnpm test` | 14 passed |
| `pnpm build` | pass |
| `pnpm tauri build --debug --no-bundle`, real debug exe, Settings driven over WebView2 remote debugging | GitHub section "Using your gh session"; Sources row "GitHub · gh session (uFleck)", green |

Screenshots (`scratchpad/relay-shots/`): `03-github.png`, `04-sources.png`.

## Known risks

- A first debug run used a temporary `APPDATA`. `gh api user` then failed (no `hosts.yml`) while `gh auth token` still
  worked through the keyring, so the label showed no login. With the real environment the login shows. The label
  degrades to "Using your gh session" whenever the login call fails; that is intended, not an error.
- Credentials are re-resolved per call, so `gh auth logout` takes effect after the cached token gets a 401.
- ClickUp and JEV rows report "stored" from the Credential Manager, not from a live call; a revoked key still shows green
  until the next real call fails.

## Review checklist

- Can the cached `gh` token outlive a `gh auth logout` for longer than one 401?
- Does anything log or serialize the `gh` token (status struct, errors, snapshot)?
- Is "Off" (grey) distinguishable from "Ok" in the rail and hover for a source with no credential?
- Does the GitHub issue import still work with no stored token and no `gh`?

## Opus verdict

## Release (2026-11-18)

Released as v0.4.0 with `scripts/release.ps1` (run from `pwsh`, not Windows PowerShell: `gh auth status` writes to stderr and aborts it there). Build 137.7 s, total 141.6 s with upload. `version.ps1 -Tag` failed at its tag step (a parameter bug), so the tag was created by hand.
