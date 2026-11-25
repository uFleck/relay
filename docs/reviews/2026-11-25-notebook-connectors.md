# Notebook action connectors

Reviewed by Codex using GPT-6.2 Sol. Base commit `0d74794`.

`src/app/Notebook.tsx` shows each unchanged task directive once with a single marker. `src/styles/main-dock.css` joins consecutive action rows with one continuous line. The line follows row height when the action wraps.

A disposable Playwright fixture checked the reported long action, sigil count, and connector continuity.

## Opus verdict
