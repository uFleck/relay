# UI-thread freeze on navigation (Projects tab)

**Status:** final
**Agent / model:** Claude Opus 5.5
**Commit(s):** branch `t3/fix-ui-thread-freeze-navigation`, on top of `1f25e65`
**Files:** `src-tauri/src/github.rs`, `src-tauri/src/commands.rs`, `src/app/Slack.tsx`, `src/app/Github.tsx`

## Cause

In Tauri 2, a command declared as a plain `fn` runs on the main thread, and that thread also pumps the webview
window. `project_git_state` was a sync command, and it spawns five `git` processes (`rev-parse` ×3, `log -1`,
`status --porcelain`). The Projects tab calls it as soon as it opens (`ProjectRepo`). Each open task sheet calls
it too (`TaskContext`). Both callers ran it again on window focus and on every `relay://change`. Every call froze the
window for about a quarter of a second, and background syncs emit a stream of change events.

The suspect named in the request, `relay_snapshot`, is not the cause. It takes about 1 ms on the real database.

## Measurements

Real database copied from `%APPDATA%\dev.relay.app` (11 projects, 99 statuses, 120 tasks, 118 links, 393 events),
with the Umbler checkouts as the linked repos. Timing came from a throwaway `#[ignore]` test in relay-core, which was
deleted before the commit.

| Call | Before (main thread blocked) | After (main thread blocked) |
|---|---|---|
| `relay_snapshot`, release, median / max | 1.10 / 1.45 ms | 0 (async pool) |
| `relay_snapshot`, debug, median / max | 3.04 / 3.93 ms | 0 (async pool) |
| `project_git_state` `C:\umbler\account` | 285 / 307 ms | 0 (blocking pool) |
| `project_git_state` `C:\umbler\app` | 248 / 305 ms | 0 (blocking pool) |
| `project_git_state` `C:\umbler\bill` | 246 / 271 ms | 0 (blocking pool) |
| `project_git_state` `C:\umbler\umbler-site` | 239 / 269 ms | 0 (blocking pool) |
| git refreshes per `relay://change` (Projects tab with a sheet open) | 2 × ~250 ms | 0 |

The git work still takes ~250 ms, but on a pool thread. The tab and the sheet show the git line when it arrives.

## What changed

- `project_git_state` is now `async` and runs the store read plus `git::state` in `spawn_blocking`.
- `relay_snapshot` and `project_set_repo` are now `#[tauri::command(async)]`. They are fast, but they take the store
  mutex, and a background sync can hold it, so they wait on a pool thread instead of the main thread.
- `useLive` takes `{ onChange: false }`. Git state is not in the store, so it no longer reloads on `relay://change`.
  It still loads on mount, on focus, on the refresh button, and after "Choose folder" / "Clear".

The frontend per-project grouping (`mixOf`: 11 projects × 120 tasks with a linear status lookup) is about 130k
comparisons per render, which is well under 2 ms. It was left unchanged.

## Checks

`cargo test -p relay-core` (119 passed, 2 ignored), `cargo fmt --check`, `cargo clippy -p relay --all-targets -D
warnings`, `cargo check`, `pnpm typecheck`, `pnpm test` (14 passed) and `pnpm build`: all pass.

## Not covered

The fix was not checked by clicking through the running app: there is no project verify skill, and no webview
profiler run was made. Other sync commands that read the store (`slack_status`, `episodes_list`, `jobs_list`) still
run on the main thread. They are cheap reads, but they could wait on the store lock. `git::state` could also drop to
two processes (`status --porcelain=v2 --branch` plus `log -1`); that change is not in this PR.

## Checklist

- [x] Policy stays in relay-core (no rule moved into adapters)
- [x] No schema or IPC contract change (the command names, arguments and return types are unchanged)
- [x] Before/after timings recorded

## Opus verdict

