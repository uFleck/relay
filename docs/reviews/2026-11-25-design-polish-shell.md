# Design polish 1/5: shell and Search

Written by Claude (Opus 5.5). Base commit `9ea47e0` (v0.12.0).

The Rail now runs the full window height. The title bar sits over the main column, is always visible and is part of the page: each view portals its title, meta line and actions into it through `PageBar`. The app name appears once, as the Rail wordmark. Tasks and Settings drop the surface card and bordered header and share the Notebook's left inset (`--page-x`), so titles line up with content. Task rows are flat like Notebook entries. Ctrl K opens Search at once with its own input; the title bar search field is gone and a search button sits beside the window controls.

Files: `src/App.tsx`, `src/app/PageBar.tsx` (new), `src/app/TitleBar.tsx`, `src/app/SearchModal.tsx`, `src/app/Notebook.tsx`, `src/app/Tasks.tsx`, `src/app/Settings.tsx`, `src/app/Rail.tsx`, `src/styles/{app,main-dock,workspace,layout-tokens}.css`, `GLOSSARY.md`.

Checks: `pnpm typecheck`, `pnpm test` (47 passed), `pnpm build`. Playwright screenshots of Notebook, Tasks, Settings and Search in light and dark against `?visual-fixture=J`.

Risks: window dragging and double-click maximize rely on `data-tauri-drag-region` on the bar, the title slot, the heading and the Rail wordmark; this was checked in the browser only, not in the Tauri window. Settings keeps its old body until the next slice.

- [x] No native dialogs
- [x] No secrets reach the frontend
- [x] `relay-core` untouched

## Opus verdict
