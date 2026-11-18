# Re-analysis with link-matched candidates

**Status:** final
**Agent / model:** Claude Sonnet 5.5
**Commit(s):** see `git log feat/reanalyze`
**Files:** `crates/relay-core/src/candidates.rs`, `crates/relay-core/src/lib.rs`, `crates/relay-core/src/store/mod.rs`,
`src-tauri/src/slack.rs`, `src-tauri/src/slack/claude.rs`, `src-tauri/src/clickup_sync.rs`, `src-tauri/src/github.rs`,
`src-tauri/src/imports.rs`, `src-tauri/src/lib.rs`, `src/ipc.ts`, `src/app/Slack.tsx`

## What changed

Problem: a group DM carried new requirements for a task, and nothing was proposed. The first sync is a backfill, so the
episode was analyzed in shadow mode ("answers stored, no action"). It was also analyzed before the ClickUp sync created
the task, and `candidates()` only searched the first five words, so the pasted ClickUp URL never mattered.

- `relay_core::candidates::pick` replaces `slack.rs::candidates`. Order: tasks whose link was pasted in the transcript
  (ClickUp `.../t/<id>` and `.../t/<team>/<id>`; GitHub `/pull/<n>` and `/issues/<n>`), then tasks matching the eight most
  frequent words, then five active tasks. Deduplicated, capped at 20.
- Links match `task_links` by `external_id` (`<id>` for ClickUp, `owner/repo#n` for `pr` and `github` links) or by URL,
  case-insensitive, trailing slash ignored, active tasks only (`Store::find_links_matching`).
- Keywords: the author prefix of each line and URLs are dropped, words are ranked by frequency, and an English plus
  Portuguese stop list applies. Each keyword is searched on its own (the old call joined five words, which FTS5 reads as
  AND) and tasks hitting more keywords rank first.
- `Store::requeue_recent(days, limit)` selects conversations active in the window whose latest job is `shadow`, or routed
  `ignore`/`review` with no proposal and was created before the newest active task. Newest first, no conversation with a
  running, `pending_agent` or unrouted `pending` job. A re-run rewrites the job's `created_at`, so it is not picked again.
- `Store::replace_job`: `create_job` keeps one job per `(conversation, watermark)`, so a live re-run of the same watermark
  would have returned the shadow job untouched. `replace_job` rewrites that row in place (state back to `pending`, ops
  cleared) unless it is in flight or already routed `change`/`new_task`.
- `analyze` takes `Existing::Keep | Replace`. Normal paths keep, re-analysis replaces and runs live (`shadow = false`).
- Commands `slack_reanalyze(conversationId)` and `slack_reanalyze_recent()` (3 days, 10 conversations, one sweep at a time).
  The ClickUp sync (created > 0), PR discovery (created > 0) and `repos_import` (imported > 0) start a background sweep,
  skipped without a JEV key.
- UI: "Re-analyze" beside "Analyze now" on the Inbox transcript sheet; "Re-analyze last 3 days" in Settings → Slack for both
  transports, each with a busy state and an inline result or error.

## Commands + results

`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `pnpm typecheck`, `pnpm build` pass.
`cargo test -p relay-core` 117 passed, 2 ignored; `cargo test -p relay` 27 passed, 2 ignored; `pnpm vitest run` 14 passed.
Real check on a copy of `relay.db` (with its WAL; the original untouched; no Slack, Claude or JEV call): the pasted link in
conversation 01M7PF80BS1TEHG5GGAQAWFC79 resolves to task 01M7PFXRHEJWAFNK5MZK3DBW54 (RLY-3DBW54, "Remover eventos
desnecessários da taxonomia do Amplitude"), which ranked first of 4 candidates. `requeue_recent(3, 10)` selected 5
conversations including that one (its latest job: shadow, route review, 0 candidates).

## Known risks

- The first sweep after upgrading re-runs every recent shadow conversation (up to 10) through JEV and the classifier. That
  is model spend, and live routing can now create review items or agent jobs for them.
- `replace_job` rewrites a row in place, so the old shadow answers are gone once the live run is stored.
- A sweep that finds another one running returns an empty report; a trigger that arrives meanwhile is not queued, the next
  trigger or the Settings button picks it up.
- `requeue_recent` compares against the newest task creation time, so a task created later than a job reselects that job
  even when the task is unrelated. The cost is one extra analysis, and the rewritten job leaves the set afterwards.
- A conversation with no job at all is not selected.
- Keyword choice still counts any frequent word (people names mentioned in the text count).

## Review checklist

- Is replacing the shadow job in place acceptable, or should the shadow answers be kept in a history row?
- Are 3 days and 10 conversations the right bounds for a background sweep after every ClickUp sync that creates a card?
- Should a `review` job with a pending human decision be redone automatically, or only on the button?

## Opus verdict
