# Release pipeline and updater, validated end to end

**Status:** complete.
**Date:** 2026-11-17 (local -03:00).
**Repo:** <https://github.com/uFleck/relay> (public, so the updater can download release assets without a token).
**Environment:** `Production` on the repo; secrets `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` set from `%USERPROFILE%\.relay\signing.key` (never printed).

## What exists now

- `scripts/release.ps1`: clean-tree check, five version fields, exact tag, existing-release probe, signing
  key from disk, `pnpm tauri build`, 420 s gate before upload, exact `X.Y.Z` installer match, `latest.json`,
  `gh release create --verify-tag`.
- `.github/workflows/release.yml` + `.github/actions/release`: the same steps for a self-hosted runner
  (label `relay-release`) or the hosted fallback. The workflow is **dispatch-only**: no runner is
  registered, so a tag push must not queue a job. Releases are cut locally with `release.ps1`.
- Updater endpoint in `tauri.conf.json`: `https://github.com/uFleck/relay/releases/latest/download/latest.json`.
  The repo is public because private release assets need auth the updater does not have.

## Measured

| Release | Build (install + build) | Total incl. upload | Installer |
| --- | --- | --- | --- |
| v0.1.0 | **123.8 s** | **128.4 s (2:08)** | 4.57 MiB + `.sig` |
| v0.1.1 | **140.9 s** | **145.4 s (2:25)** | 4.56 MiB + `.sig` |

Both are under the 5:00 target with well over a minute of slack.

## Updater test (a real install, not a simulation)

1. Installed `Relay_0.1.0_x64-setup.exe` silently; `%LOCALAPPDATA%\Relay\relay.exe` reported
   `plugin:app|version = 0.1.0`, updater pointing at the public `latest.json`.
2. Published v0.1.1 (same signing key).
3. In the installed app: Settings → **Check for updates** → `Relay 0.1.1 is available`.
4. **Download** → the plugin downloaded the installer and **verified the signature**
   (`Relay 0.1.1 is downloaded and signed`), then offered **Restart to update**.
5. Clicked it: the app exited, the NSIS updater replaced the binary, the app relaunched.
6. After relaunch: `%LOCALAPPDATA%\Relay\relay.exe` FileVersion/ProductVersion **0.1.1** and
   `plugin:app|version = 0.1.1`; Settings shows `Relay 0.1.1 · up to date as far as the last check knows`.

Evidence: `Settings` screenshot after the restart (Relay 0.1.1, green MCP, tokens and snippets).

## Notes and risks

- The self-hosted workflow has **never run**: no runner with `relay-release` exists. The local path is the
  measured one. If a runner is registered later, dispatch the workflow from the tag ref.
- `release.ps1` hard-codes the repo URL only in `latest.json`; it matches `origin`.
- The installed app and the repo share `%APPDATA%\dev.relay.app\relay.db`, which is intended (one Relay
  per user). Uninstalling does not delete the database.
- `version.ps1`'s `cargo metadata --offline` can warn when a transitive package is not in the local cache;
  it still rewrites the local package entries and the next build completes the lock. Observed for 0.1.1
  with `async-executor`; the build was fine.
- The update is a full NSIS installer (passive mode), so there is a brief window with no Relay process.
