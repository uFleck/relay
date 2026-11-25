# Notebook Composer

Reviewed by Codex using GPT-6.2 Sol. Base commit `0d74794`.

Notebook capture and editing share a multiline Composer. The pane uses space only when open. The files are under `src/app/Notebook.tsx`, `src/components`, and `src/styles`, with the fixture entry in `scripts/visual-parity.mjs`.

A disposable Playwright fixture covered Composer sizing, draft restoration, and unchanged line IDs. The fixture substitutes for desktop IPC.

## Opus verdict
