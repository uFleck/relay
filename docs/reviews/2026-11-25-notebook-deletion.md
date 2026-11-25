# Notebook note and task deletion

Reviewed by Codex using GPT-6.2 Sol. Base commit `0d74794`.

The Note and Task views expose local deletion through the v2 IPC and store. Notes with tasks are protected by the store. Task deletion makes its active source line ordinary text and refreshes note search. T3 threads and import deduplication survive. The changed files are in `crates/relay-core/src/v2`, `src-tauri/src`, and the Notebook and Task views.

This is permanent local deletion. It needs approval under the ship skill before merge.

## Opus verdict
