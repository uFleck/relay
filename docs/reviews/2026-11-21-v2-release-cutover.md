# v2 packaged Windows cutover (#43)

Status: release published; live install and end-to-end cases deferred with concrete causes below.

- **Agent / model** - OpenCode session on `opencode-go/hy4-flash-free`, continuing the v2 rollout. No subagent was used. The signing key and password were read by the release script from outside the repo and were never printed or committed. The user chose "publish v0.9.0 now" when asked about the release posture.

- **Commit(s)** - Base `6cba32c` (merge of #47). Local commits: `440eb3d` (release preflight fix), `d6f4bc2` (version bump), and this ledger at the final local HEAD. Tag `v0.9.0` points at `d6f4bc2` and is pushed.

- **Files** - `scripts/release.ps1` (capture `gh` stderr without tripping `$ErrorActionPreference = 'Stop'`), version fields in `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `crates/relay-core/Cargo.toml` and `[workspace.package]` in `Cargo.toml`, plus the refreshed `Cargo.lock`. This ledger added.

- **What changed** - The release preflight now inspects `gh auth status` and `gh release view` output without treating their stderr as a terminating PowerShell error; a stale secondary account no longer blocks a healthy active login. Versions move from 0.8.0 to 0.9.0 so the v2 build can publish as a new stable release without clobbering v0.8.0. The fresh-start decision from #21 applies: no v1 migration, backup or rollback path.

- **Commands + results** (Windows):
  | Command | Result |
  | --- | --- |
  | `release.ps1 0.8.0 -DryRun` before fix | Aborted: `gh auth status` stderr tripped `ErrorActionPreference = Stop`. |
  | `release.ps1 0.8.0 -DryRun` after fix | Completed: version fields match, tag mismatch and existing v0.8.0 release reported, planned commands printed. Pass. |
  | `pnpm tauri build` at 0.8.0 with warm target, signing env set | 396.2 s; `Relay_0.8.0_x64-setup.exe` (5.42 MiB) and `.sig` produced. Pass. |
  | `cargo test -p relay-core` | 174 passed, 0 failed, 2 ignored. Pass. |
  | `pnpm typecheck` / `pnpm test` | Clean / 7 files, 44 tests passed. Pass. |
  | `version.ps1 -Version 0.9.0` | Five version fields and `Cargo.lock` updated; no tag created. Pass. |
  | `release.ps1 0.9.0` with cold worktree target | Build path 492.5 s, over the 420 s gate; correctly refused to publish. Pass (gate worked). |
  | `release.ps1 0.9.0` with `CARGO_TARGET_DIR=C:\Projects\relay\target` | Build 231.5 s, budget gate 249.6 s, total 265.8 s; installer, `.sig` and `latest.json` written; `gh release create` published. Pass. |
  | `gh release view v0.9.0` | Stable (not prerelease), assets: `latest.json` (838 B), installer (5,678,438 B), signature (432 B). Pass. |
  | GET `/releases/latest/download/latest.json` | `version=0.9.0`, URL points at the v0.9.0 installer, signature present. Pass. |

- **Known risks / exact gaps** - No NSIS installer was actually run: a packaged fresh install and the 0.8.0 → 0.9.0 upgrade opening an empty Notebook with fresh setup were not exercised, so the updater's runtime self-update is not verified on hardware. No live end-to-end Note → approved Brief → returned T3 run was driven, and no live Slack/GitHub/ClickUp/OpenCode call was made; those depend on a desktop session and configured providers. A cold-target build exceeds the 420 s gate, so the warm shared target (or the self-hosted runner cache) remains required. The published tag points at the branch commit `d6f4bc2`; the PR merge brings that commit onto `master` without moving the tag.

- **Review checklist** - (1) Preflight no longer aborts on `gh` stderr. (2) All version fields and the lock agree on 0.9.0. (3) Signed installer and `.sig` match the exact version. (4) Release is stable and `latest.json` points at it. (5) Budget gate refuses a cold build and accepts a warm one. (6) Live install, upgrade, T3 and provider cases are explicitly deferred, not claimed as verified.

- **Opus verdict** - (left empty for the reviewer)
