# Design polish 5/6: Dock capture bar

Written by Claude (Opus 5.5). Base: `master` after #75; independent of the Vim slice.

The Dock becomes a capture bar: the Composer is always open and focused on summon, with one hint row (ctrl+↵ send, esc dismiss, the summon chord, Open Relay). Removed: the wordmark header, the pencil toggle and "Close composer", the Do not disturb button, the update banner, the "N tasks need you" line, the waiting list and its inline Brief/Grill panel, and the browser-only "Simulate change" bar.

Do not disturb is removed end to end. Nothing read the flag after the v2 Slack sync left, so the tray item, the `set_dnd` command, the `dnd` field on the hover flags and `setDnd` go. The hover-only update plumbing goes with the banner: the `check_for_updates` command, `requestUpdateCheck`, `followUpdates`, `requestInstall` and the `update://state|request|install` events. The tray's "Check for updates" still works through `update://check`.

The scrollbar came from `.dock-scroll { overflow: auto }` inside a window Rust already sizes to the measured content; it is now `overflow: hidden`. The Dock is 640px wide (`DOCK_WIDTH_CSS`, `DOCK_PREFERRED_WIDTH`); the geometry tests now pass their width explicitly.

Files: `src/hover/HoverApp.tsx`, `src/hover/hover.css`, `src/hover/dock.ts` + test, `src/styles/dock.css`, `src/ipc.ts`, `src/updater.ts`, `src-tauri/src/hover.rs`, `src-tauri/src/lib.rs`.

Checks: `pnpm typecheck`, `pnpm test` (49 passed), `pnpm build`, `cargo test -p relay --lib` (48 passed; 3 `process::tests` failed under parallel load and pass alone, untouched by this diff). Playwright screenshots of `hover.html` in light and dark.

Risks: summon, focus and resize were not exercised in the real Tauri window. Append (adding Dock text to an existing note) is the next slice.

- [x] No native dialogs
- [x] No secrets reach the frontend
- [x] `relay-core` untouched

## Opus verdict
