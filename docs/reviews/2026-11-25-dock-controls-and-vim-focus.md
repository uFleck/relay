# Dock footer and Vim selection

Reviewed by Codex (GPT-6.2 Sol). Base commit 90b0a6a. Fix commit 3cccb09.

- The Dock focus hint sits inside the measured footer. Shortcut hints and Open Relay can wrap as the available width decreases.
- Pointer movement, presses, and wheel input clear the main window's Vim selection and cancel any pending Vim key sequence.

Files reviewed include `src/app/useVim.ts`, `src/hover/HoverApp.tsx`, `src/hover/hover.css`, and `src/styles/dock.css`.

- [x] The Chromium reproduction failed before the fix and passed after it. It checked the Dock footer at 640 px and 320 px with a simulated Tauri focus event, and covered Vim selection with mouse movement, presses, scrolling, and a return to keyboard input.
- [x] All eight tests passed in `pnpm test src/hover/dock.test.ts src/vim.test.ts`.
- [x] `pnpm build` and `git diff --check` passed.

Native Windows focus and resizing still need coverage.

## Opus verdict
