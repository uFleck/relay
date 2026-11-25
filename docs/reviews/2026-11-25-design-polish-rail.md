# Design polish 3/5: Rail update notice and hints

Written by Claude (Opus 5.5). Stacked on the Settings slice.

The updater moves fully into the Rail. One line names the next step (`0.13.0 available`, `Downloading 42%`, `Restart for 0.13.0`, `Update failed · retry`) and runs it on click. Hover or focus opens a custom tooltip with the changelog of every release between the running version and the offered one. The version label at the bottom of the Rail runs a manual check and reports "up to date" or "check failed" for a few seconds. Settings drops its Updates section and `UpdateActions` is deleted. The hint block gains `ctrl+k search`.

The updater keeps the changelog through `downloading` and `ready` (new `notes` field on both states). Download progress is now shown as a percentage; the old Rail line printed the 0–1 fraction with a `%` sign.

Files: `src/app/UpdateNotice.tsx` (new), `src/app/changelog.ts` + test (new), `src/app/Rail.tsx`, `src/App.tsx`, `src/app/Settings.tsx`, `src/app/UpdateActions.tsx` (deleted), `src/updater.ts`, `src/styles/{app,main-dock}.css`.

Checks: `pnpm typecheck`, `pnpm test` (49 passed, 2 new for the changelog parser), `pnpm build`. Playwright screenshots of the notice with its tooltip open in light and dark, using `?visual-fixture=J&visual-update=available`.

Risks: the Dock still mirrors updater state through events; the new `notes` field is additive. The Dock's own update banner is removed in the Dock slice.

- [x] No native dialogs
- [x] No secrets reach the frontend
- [x] `relay-core` untouched

## Opus verdict
