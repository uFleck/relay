# v2 public docs and examples (#42)

Status: complete locally; independent review pending.

- **Agent / model** - OpenCode session on `opencode-go/hy4-flash-free`, from the handoff that continued the v2 rollout. No subagent, push, PR, issue comment/close, tag, release or credential change was made. The worktree is `C:\Users\Pichau\.t3\worktrees\relay\relay-v2-42-docs-cleanup` on `relay-v2/42-docs-cleanup`, branched from `master` after #40 merged. The main checkout was not modified.

- **Commit(s)** - Base `9d3dd5f` (merge of #46). Implementation and this ledger are the local commits on this branch; the final HEAD is the reviewed SHA.

- **Files** - Deleted `docs/HANDOFF.md`, `docs/SLACK_SETUP.md`, `design/main.html`, `design/hover.html`, `design/README.md` and all seven `design/redesign/` HTML files plus its README. `design/app-icon.png` is kept: it is the source for the generated icon set, not a mockup. Rewrote the README introduction, added a "Using Relay v2" section, corrected the Database/Releases/Layout text, and marked `docs/PLAN.md` as v1 history with Decision 4 reversed. Removed stale mockup references from `src/hover/hover.css`, `src-tauri/src/hover.rs`, `src/styles/app.css` and `src/styles/layout-tokens.css`. Neutralized personal names in live fixtures: three GitHub review logins in `crates/relay-core/src/github.rs` and project-owner fixtures in `crates/relay-core/src/v2/tests.rs`. Added this ledger and its index row.

- **What changed** - R6: public examples and fixtures now use neutral owners and reviewers; the project's own repo/updater URLs and attribution are unchanged. R7: the old handoff and Slack app-token setup guides are retired, the superseded main/hover/redesign mockups are gone, and current guidance describes v2 provider setup, the three views, Composer directives, Brief approval, T3 progress and Note-only Import. PLAN remains as v1 history with an explicit superseded notice.

- **Commands + results** (Windows) - `cargo test -p relay-core`, `cargo check -p relay`, `pnpm typecheck`, `pnpm test` and `git diff --check` were run after the edits; results are recorded in the final handoff. Deleted files were removed with `git rm`; no code behaviour changed.

- **Known risks / exact gaps** - `docs/PLAN.md` and the historical ledgers still describe v1 internals by design; they are marked as history rather than rewritten. `docs/SPEC-v2.md` is not on this branch, so the README links the public decision-map issue instead of a local file. The retired mockups are not replaced here: the chosen Dock + Lens direction is implemented in the app itself. No live Windows UI smoke was run for the rewritten guidance; Spawner/Hopper were not started on anyone's initiative.

- **Review checklist** - (1) No v1 setup guide or superseded mockup remains. (2) Public fixtures use neutral owner/reviewer names and the repo/updater URLs are intact. (3) README describes v2 setup and the Note-to-Task workflow. (4) PLAN identifies Decision 4 as superseded. (5) No stale code comment points at a deleted mockup. (6) All four required checks pass.

- **Opus verdict** - (left empty for the reviewer)
