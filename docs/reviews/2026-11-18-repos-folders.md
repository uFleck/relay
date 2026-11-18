# Several repository folders

**Status:** final
**Agent / model:** Claude Opus 5.5
**Commit(s):** uncommitted at time of writing
**Files:** `crates/relay-core/src/repos.rs`, `src-tauri/src/imports.rs`, `src/ipc.ts`, `src/app/Github.tsx`

## What changed

Asked for: the folder is a root that holds clones (like `C:\umbler`), not a repository, and several roots can be
tracked.

- A non-repo root already worked: `scan_folder` checks the root and its direct subfolders. A real scan of
  `C:\umbler` finds 10 GitHub clones. The note under the picker now says so.
- `repos::scan_folders(roots)`: scans every root and keeps one checkout per repo across all of them (same
  main-checkout-then-shortest-path rule). `scan_folder` is now a one-root wrapper.
- `repos_scan` / `repos_import` take `folders: Vec<String>`; a folder that is gone fails with its path.
- Settings → GitHub → "Repositories folders": a list with Add folder… and Remove per row; the scan and the import
  cover all of them. The list lives in `localStorage` (`relay.reposFolders`); the old single `relay.reposFolder` is
  carried over on first read.

## Checks

`cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` (core 119, app 27),
`pnpm typecheck`, `pnpm vitest run` (14), `pnpm build`: all pass. New test
`scan_of_several_folders_merges_them_and_keeps_one_checkout_per_repo`.

## Not covered

Not clicked in the running app. Folders are only remembered per machine in the webview, not in the database; the PR
sync does not need them, since it follows the imported projects.
