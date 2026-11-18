# No console window per git call

**Status:** final
**Agent / model:** Claude Opus 5.5
**Commit(s):** uncommitted at time of writing
**Files:** `crates/relay-core/src/git.rs`

## What changed

Problem: on Windows, Relay is a GUI process, so every `git` child without `CREATE_NO_WINDOW` opens a console window.
`git::run` lacked the flag (`gh`, `claude` and the analyst already set it). The 0.5.3 folder scan runs one
`git remote get-url origin` per subfolder of every chosen folder, so a scan or import of `C:\umbler` flashed dozens of
terminals. The task sheet's git line (4 calls) flashed too.

- `git::run` sets `CREATE_NO_WINDOW` on Windows. It is the only non-test `git` spawn.

## Checks

`cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` (core 119,
app 27): pass.

## Not covered

Not observed in the installed app; the flag is the same one `github.rs` uses for `gh`, which shows no windows.
