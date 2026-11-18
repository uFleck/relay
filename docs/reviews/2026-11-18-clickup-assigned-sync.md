# ClickUp: track only cards assigned to me

**Status:** final
**Agent / model:** Claude Sonnet 5.5
**Commit(s):** see `git log feat/clickup-mine`
**Files:** `crates/relay-core/src/clickup.rs`, `crates/relay-core/src/clickup_sync.rs`, `crates/relay-core/src/lib.rs`,
`crates/relay-core/src/store/mod.rs`, `src-tauri/src/clickup_sync.rs`, `src-tauri/src/imports.rs`, `src-tauri/src/lib.rs`,
`src/app/Import.tsx`, `src/app/Views.tsx`, `src/ipc.ts`

## What changed

- The old list import pulled one whole list (106 cards) and mapped statuses by exact name. ClickUp statuses carry emoji
  ("👨‍💻 doing"), so everything became Todo. `relay_core::clickup_sync::clickup_category` now maps by category from a
  keyword table (status `type` done/closed wins). The list import uses it too.
- `ClickUpClient` gains `me`, `workspaces`, `spaces` and `assigned_tasks` (assignee filter, subtasks, 20-page cap with a
  `truncated` flag). `ClickUpTask` carries `status_type` and `list_name`.
- `reconcile` is pure: create unseen cards, move a task only when the ClickUp status changed since the last sync (or the
  link was never mapped by this sync, which fixes the old import once), unarchive a card assigned again, archive linked
  tasks that are no longer assigned. A truncated list archives nothing. Finished cards are not imported.
- `src-tauri/src/clickup_sync.rs`: scope in `settings` (`clickup.workspaceId`, `clickup.spaceId`, `clickup.projectId`),
  commands `clickup_workspaces`, `clickup_spaces`, `clickup_scope`, `clickup_set_scope`, `clickup_sync_now`, a 10-minute
  schedule (first run 30 s after start) and a guard against overlapping runs. A failure turns the ClickUp Sources row
  Down with the error.
- Settings → ClickUp: workspace, space ("All spaces I can see" by default), Relay project, Save and sync, Sync now, a
  result line and the 10-minute note.
- Real account finding: the token's user is a guest ("Shared with me" lists), `GET /team/{id}/space` returns no spaces,
  so the space is optional and the workspace-wide assigned query is the default path.

## Commands + results

`cargo test -p relay-core` 97 passed; `cargo test -p relay --lib` 27 passed, 2 ignored (one is the real-API test);
clippy `-D warnings`, `cargo fmt --check`, `pnpm typecheck`, `pnpm vitest run` (14), `pnpm build` pass.
Real read-only run (`real_account_assigned_cards_by_status_and_category`, GET only): 286 assigned cards, 272 `done`,
3 mapped to todo, 7 deploying, 3 active, 1 review. The app was not launched and `relay.db` was not touched.

## Known risks

- The first sync archives every ClickUp-linked task in the target project that is not assigned to the user. That is the
  intent for the 106 imported cards, but it is a bulk change; archived tasks are recoverable.
- Existing links from the old import have their status re-mapped once, so a status the user edited locally on them is
  replaced one time.
- Guest access: ClickUp returns "Shared with me" cards only through the workspace-wide query. Other scopes are untested
  against real data.
- `list_links_in_project` is unbounded; fine at this scale.
- No retry on a 5xx. One transient 500 was seen while a request carried an empty `space_ids[]`, which is now never sent.

## Review checklist

- Is skipping `done` cards on first import right, or should recently finished cards appear?
- Does archiving on "not assigned" match what the user expects when a card moves to someone else temporarily?
- Is a 10-minute poll against the ClickUp rate limit (100/min) safe with up to 20 pages per run?

## Opus verdict
