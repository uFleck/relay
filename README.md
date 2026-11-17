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

The updater plugin and endpoint (`https://github.com/uFleck/relay/releases/latest/download/latest.json`) are configured; the in-app update state/UI is not built yet. The configured `plugins.updater.pubkey` is the placeholder `REPLACE_WITH_TAURI_UPDATER_PUBKEY` until the real public key is added.

```powershell
# 1. Bump all four version fields
scripts/version.ps1 -Version 0.1.0

# 2. Commit the bump, then tag that commit (version.ps1 is idempotent)
git add -A
git commit -m "Release v0.1.0"
scripts/version.ps1 -Version 0.1.0 -Tag
git push origin v0.1.0

# 3. Build, sign, publish from a warm target/ (must be a clean tree)
scripts/release.ps1 0.1.0
scripts/release.ps1 0.1.0 -SkipInstall
scripts/release.ps1 0.1.0 -DryRun
```

`release.ps1` reads the private key from `TAURI_SIGNING_PRIVATE_KEY_PATH` or `%USERPROFILE%\.relay\signing.key`, writes `latest.json` (version, pub_date, `windows-x86_64` signature + URL), and uploads the installer, its `.sig` and `latest.json` with `gh release create`. It prints per-step stopwatch timings and fails if the total is over 420s. It never generates or prints a key.

If Windows blocks the scripts (`running scripts is disabled on this system`), use `pwsh -NoProfile -File scripts/release.ps1 0.1.0` or `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/release.ps1 0.1.0`.

```powershell
# Once: generate the key pair. Prints ONLY the public key (base64).
scripts/generate-signing-key.ps1
```

- Private key: `%USERPROFILE%\.relay\signing.key` (outside the repo; `*.key` is gitignored).
- Public key file: `%USERPROFILE%\.relay\signing.key.pub` (the script prints its base64 contents).
- Generated password: `%USERPROFILE%\.relay\signing.key.password`, read automatically by `release.ps1`. If you pass `-Password`, set `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` instead.
- Public key value: paste the printed base64 into `src-tauri/tauri.conf.json` → `plugins.updater.pubkey`.
- CI: store the private key in the `Production` environment as `TAURI_SIGNING_PRIVATE_KEY` (and its password as `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`).

`.github/workflows/release.yml` runs on `v*` tags on `[self-hosted, Windows, X64, relay-release]` with `C:\relay-cache\target` and `C:\relay-cache\pnpm-store`, so the build is warm. The hosted fallback is only `workflow_dispatch` with `runner=hosted`, dispatched from the version tag; it will not hit 5 minutes. The local/self-hosted path is the fast path.
