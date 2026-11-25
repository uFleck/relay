# Design polish 2/5: Settings

Written by Claude (Opus 5.5). Stacked on the shell slice (`ea50efe`).

Settings becomes one column at the Notebook's width. Each section has a mono heading over hairline rows, with the setting on the left and its control on the right. Providers are one row each (name, short reason, status pill) and open inline to edit; a recovery route (`?provider=`) opens and focuses its row. The provider forms, their write commands and the credential handling are unchanged.

Files: `src/app/Settings.tsx`, `src/styles/app.css`.

Checks: `pnpm typecheck`, `pnpm test` (47 passed), `pnpm build`. Playwright screenshots of Settings and of `?provider=github` in light and dark against `?visual-fixture=J`.

Risks: a closed provider row unmounts its form, so a half-typed value is dropped when the row closes. Credential inputs were already cleared on submit, so no secret is kept longer than before.

- [x] No native dialogs (the folder picker is unchanged)
- [x] No secrets reach the frontend
- [x] `relay-core` untouched

## Opus verdict
