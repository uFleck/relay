# Relay

A local-first Windows desktop app that keeps one task graph for work spread across Slack DMs, ClickUp/GitHub, git/PRs and AI sessions. Local SQLite is the source of truth; integrations are optional projections. The plan is in [docs/PLAN.md](docs/PLAN.md).

Status: scaffold. It has the Tauri main window, tray, single instance, autostart, the `relay-core` model/SQLite store, and the updater/release pipeline. The hover window and the real UI are not implemented yet; the static design mockups live in `design/`.

## Stack

Tauri 2 + React 19 + TypeScript + Vite 8, pnpm 10, Node 24.4.1, Rust 2024. The core crate (`crates/relay-core`) is Tauri-free; `cargo test -p relay-core` is the fast loop.

## Development

Requires Node 24.4.1 (`.node-version`), pnpm 10, the Rust toolchain and WebView2.

```sh
pnpm install
pnpm tauri dev          # desktop app
pnpm dev                # frontend alone (port 1420)
pnpm typecheck
pnpm test
pnpm build
cargo test -p relay-core
cargo check
```

Tauri needs `dist/` for `cargo check`/`cargo build` because `generate_context!` embeds `frontendDist`; run `pnpm build` first (CI does).

## Layout

```
crates/relay-core/      model + SQLite store, no Tauri
  migrations/           versioned SQL, applied through PRAGMA user_version
src-tauri/              Tauri shell: main window, tray, plugins, DB setup
src/                    React scaffold (tokens.css copied from Hopper)
scripts/                version, release and signing-key helpers
.github/actions/release composite release steps
.github/workflows/      ci.yml + release.yml
```

## Database

The app opens `%APPDATA%\dev.relay.app\relay.db` at startup, enables WAL and foreign keys, and applies `crates/relay-core/migrations/0001_init.sql`. Tests use an in-memory database. The schema mirrors [docs/PLAN.md](docs/PLAN.md) section 2, including FTS5 over `tasks(title, notes)`.

## Releases

The updater plugin and endpoint (`https://github.com/uFleck/relay/releases/latest/download/latest.json`) are configured. A local key pair was generated on 2026-11-17 and its public key is already in `plugins.updater.pubkey`; the in-app update state/UI is still being built.

```powershell
# 1. Bump every version field: package.json, src-tauri/tauri.conf.json, both
#    Cargo [package] versions, [workspace.package].version and Cargo.lock
scripts/version.ps1 -Version 0.1.0

# 2. Commit the bump, then tag that commit (version.ps1 is idempotent)
git add -A
git commit -m "Release v0.1.0"
scripts/version.ps1 -Version 0.1.0 -Tag
git push origin v0.1.0        # the tag must be on the remote before release.ps1 runs

# 3. Build, sign, publish from a warm target/ (must be a clean tree)
scripts/release.ps1 0.1.0
scripts/release.ps1 0.1.0 -SkipInstall
scripts/release.ps1 0.1.0 -ForceOverBudget   # publish even if the build path passed 420s
scripts/release.ps1 0.1.0 -DryRun
```

`release.ps1` runs this preflight before it builds or uploads anything:

- the working tree is clean;
- `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` `[package]`, `crates/relay-core/Cargo.toml` `[package]` and `Cargo.toml` `[workspace.package]` (when present) all equal the version argument; a partial bump fails and names every mismatched file;
- the `v<version>` tag exists locally;
- `gh` is installed and can resolve the repository (`origin`, or `GH_REPO=owner/repo`), and `gh release view <tag>` reports the release as not found, so an existing release is never clobbered;
- the signing key exists.

It reads the private key from `TAURI_SIGNING_PRIVATE_KEY_PATH` or `%USERPROFILE%\.relay\signing.key`, writes `latest.json` (version, pub_date, `windows-x86_64` signature + URL), and uploads the installer, its `.sig` and `latest.json` with `gh release create`. The installer must match the version exactly (`<product>_<version>_<arch>-setup.exe` plus `.sig`); there is no newest-`.exe` fallback. It prints per-step stopwatch timings, warns at 300s and fails above 420s after the build and **before** `gh release create`; the upload time is excluded from that gate, and `-ForceOverBudget` publishes anyway. The tag must already be on the remote: `gh release create` runs with `--verify-tag`, so a local-only tag fails instead of creating a release at the default branch.

`version.ps1` refreshes `Cargo.lock` with `cargo metadata --offline --format-version 1` after changing a Cargo version. If that offline command fails (cold registry cache), it prints the error and the bump continues; the next build updates the lock. It never uses the network.

### Prereleases

A version with a prerelease suffix is published as a GitHub prerelease by both `release.ps1` and the CI action (`--prerelease`, title `Relay vX.Y.Z-pre (pre-release)`):

```powershell
scripts/version.ps1 -Version 0.2.0-beta.1
git add -A
git commit -m "Release v0.2.0-beta.1"
scripts/version.ps1 -Version 0.2.0-beta.1 -Tag
git push origin v0.2.0-beta.1
scripts/release.ps1 0.2.0-beta.1
```

`latest.json` is still written and uploaded with the prerelease, but GitHub excludes prereleases from `/releases/latest`, so installed apps keep updating from the newest stable release. Testers install the beta manually.

If Windows blocks the scripts (`running scripts is disabled on this system`), use `pwsh -NoProfile -File scripts/release.ps1 0.1.0` or `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/release.ps1 0.1.0`.

```powershell
# Already generated locally on 2026-11-17. Re-run only to rotate the pair.
# It prints ONLY the public key (base64); installed apps stop accepting updates
# signed by the old key, so rotate deliberately.
scripts/generate-signing-key.ps1 -Force
```

- Private key: `%USERPROFILE%\.relay\signing.key` (outside the repo; `*.key` is gitignored).
- Public key file: `%USERPROFILE%\.relay\signing.key.pub` (the script prints its base64 contents).
- Generated password: `%USERPROFILE%\.relay\signing.key.password`, read automatically by `release.ps1`. If you pass `-Password`, set `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` instead.
- Public key value: paste the printed base64 into `src-tauri/tauri.conf.json` → `plugins.updater.pubkey`.
- CI: store the private key in the `Production` environment as `TAURI_SIGNING_PRIVATE_KEY` (and its password as `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`).

`.github/workflows/release.yml` runs on `v*` tags on `[self-hosted, Windows, X64, relay-release]` with `C:\relay-cache\target` and `C:\relay-cache\pnpm-store`, so the build is warm. The hosted fallback is only `workflow_dispatch` with `runner=hosted`, dispatched from the version tag; it will not hit 5 minutes. `.github/actions/release` mirrors the same version-field check, prerelease flag, exact installer match and pre-upload budget gate (hosted is not failed by the 420s gate). The local/self-hosted path is the fast path.
