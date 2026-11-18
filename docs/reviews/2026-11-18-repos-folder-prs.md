# Repositories folder and PR discovery

**Status:** final
**Agent / model:** Claude Sonnet 5.5
**Commit(s):** see `git log feat/repos-folder` (one commit, subject "Import a repositories folder and discover your PRs")
**Files:** `crates/relay-core/src/repos.rs`, `crates/relay-core/src/github.rs`, `crates/relay-core/src/git.rs`, `crates/relay-core/src/lib.rs`,
`src-tauri/src/github.rs`, `src-tauri/src/imports.rs`, `src-tauri/src/lib.rs`,
`src/app/Github.tsx`, `src/app/Import.tsx`, `src/ipc.ts`, `src/types.ts`, `src/styles/app.css`

## What changed

- Settings → GitHub has a "Repositories folder" block. "Choose folder…" (the folder picker, the only dialog) scans the
  folder and its immediate subfolders for clones whose `origin` is on github.com. "Import N repos" creates one project per
  clone that no project covers yet (name = repo name, unique slug, `repo_path` = clone). The last folder is remembered in
  `localStorage` per viewer.
- `poll_once` (every 3 minutes, and "Sync pull requests now") now starts with discovery: two search calls
  (`author:@me`, `review-requested:@me`), then a task per open PR in a repo that a project tracks. Author tasks keep the PR
  title, review requests get `Review: `. Tasks land in the project's Todo status with `source = github` and a `pr` link, so
  the existing poller fills state, checks and review. Discovery runs before the "no PR links" early return, needs a token
  (stored or `gh`), and its errors join the poll errors without stopping PR polling.
- Policy (remote parsing, scan, slugs, which PRs become tasks, idempotent import) lives in `relay-core::repos`; the Tauri
  commands `repos_scan` and `repos_import` are thin.

## Commands + results (2026-11-18)

| Command | Result |
| --- | --- |
| `cargo test -p relay-core` | 97 passed, 2 ignored (`real_scan`, `real_search`) |
| `cargo test -p relay --lib` | 26 passed, 1 ignored |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo fmt --all -- --check` | clean |
| `pnpm typecheck`, `pnpm vitest run`, `pnpm build` | pass (14 tests) |
| `real_scan` on `C:\umbler` | 16 clones, all `ublr/*` |
| `real_search` with `gh auth token` | 3 open PRs, all Author, 0 review-requested |

The desktop UI was not launched against the real `relay.db`.

## Known risks

- Several clones of one repo (`account`, `account-1725`, ...) each become a project. A PR is assigned to the first project
  with that origin, and the link-URL check keeps the others from duplicating it.
- Path matching for "already a project" is case-insensitive on Windows only; other platforms compare exactly.
- Discovery runs `git remote get-url origin` per project on every poll.
- Search covers one page (100) per query; more open PRs than that are not discovered.
- A PR that is closed or merged stops being polled as before; its task is not removed.

## Review checklist

- Does a second poll or a second import create nothing new?
- Does discovery with no token skip silently, and an API failure still let linked PRs poll?
- Is a PR in a repo with no project ignored?
- Does any error or log text include a token?

## Opus verdict
