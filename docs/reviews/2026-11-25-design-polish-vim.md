# Design polish 4/5: Vim mode

Written by Claude (Opus 5.5). Stacked on the Rail slice.

Vim mode makes the main window keyboard-driven and is on by default. Normal mode (no text field focused) moves a selection through the current view's items with j/k, gg/G and ctrl+d/u; enter/o opens, e edits a note, dd opens the in-app delete confirmation, i/a starts a new note, gn/gt/gs switch views, / opens Search and ? opens a cheat sheet. Insert mode is any focused field; Esc blurs it. Search moves with ctrl+j/ctrl+k when Vim mode is on. The Rail shows a NORMAL/INSERT badge and a `? keys` hint. The Composer is unchanged.

The key grammar is a pure function (`src/vim.ts`) with tests. The DOM side (`src/app/useVim.ts`) finds items by `data-vim-item` and their actions by `data-vim-open`, `data-vim-edit` and `data-vim-delete`, so each view keeps its own buttons and Vim mode only clicks them. The toggle lives in Settings → Keybinds and is stored as `vim` in `keybinds.json`; a file written before this change reads as `vim: true`.

Files: `src/vim.ts` + test (new), `src/app/useVim.ts` (new), `src/app/VimHelp.tsx` (new), `src/App.tsx`, `src/app/{Rail,Notebook,Tasks,Settings,SearchModal}.tsx`, `src/components/DeleteConfirmation.tsx`, `src/keybinds.ts`, `src/styles/app.css`, `src-tauri/src/keybinds.rs`.

Checks: `pnpm typecheck`, `pnpm test` (54 passed, 5 new), `cargo test -p relay --lib keybinds` (2 new, passed), `pnpm build`. Playwright drove j/k on Notebook, `?`, `gt` + j/k on Tasks, `gs` + j/k on Settings and `i` into Insert mode, in light and dark.

Risks: Enter is left to the browser when a button or link has focus, so Tab navigation keeps working. Vim keys never fire inside a text field, Search, the cheat sheet or a delete confirmation.

- [x] No native dialogs (dd opens the existing in-app confirmation)
- [x] No secrets reach the frontend
- [x] `relay-core` untouched; the Tauri change is a config field

## Opus verdict
