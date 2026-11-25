# Windows launcher discovery

Reviewed by Codex using GPT-6.2 Sol. Base commit `0d74794`.

OpenCode resolves the native npm executable under the trusted install roots. Settings, mapping checks, and dispatch share the T3 launcher and process runner. The changed files are `opencode.rs`, `settings.rs`, `t3.rs`, and `t3_cli.rs` under `src-tauri/src`.

The resolver probe found and launched OpenCode 2.4.1 and T3 Code 0.1.12 nightly on Windows. A read-only T3 CLI call listed threads for the registered Relay project.

## Opus verdict
