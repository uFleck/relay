# Review actions for Slack jobs

**Status:** final
**Agent / model:** Claude Opus 5.5
**Commit(s):** see the 0.5.2 release commit and its parent
**Files:** `crates/relay-core/src/store/mod.rs`, `src-tauri/src/slack.rs`, `src-tauri/src/lib.rs`, `src/ipc.ts`,
`src/app/Slack.tsx`

## What changed

Problem: a job routed `review` stays `pending` until a human settles it, but Review → From Slack only offered a
Transcript button, and the transcript sheet read any `pending` job as "Analysis running". The group DM for RLY-3DBW54
sat there with the right task chosen (confidence 1.0) and no way to act.

- `Store::resolve_review_job(id, send)`: only a `pending` job routed `review` qualifies. `send` sets route `change` and
  state `pending_agent`; dismiss sets route `dismissed` and state `done`. Neither route is selected by `requeue_recent`.
- Command `job_resolve_review(id, send)`: sets the episode to `waiting` (send) or `analyzed` (dismiss) and starts the
  reviewer on send. `jobResolveReview` in `ipc.ts`.
- Review → From Slack rows get "Send to reviewer" and "Dismiss", with a busy spinner and an inline error.
- `EpisodeSheet`: a `pending` job routed `review` shows a disabled "In Review" button instead of "Analysis running".

## Checks

`cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` (core 118, app 27),
`pnpm typecheck`, `pnpm vitest run`, `pnpm build`: all pass. New test
`a_review_job_resolves_once_and_then_leaves_the_requeue`.

## Not covered

The buttons were not clicked in the running app, and the reviewer's output on the RLY-3DBW54 job was not observed.
