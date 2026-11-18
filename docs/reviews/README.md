# Review ledger

One factual record per work package, so a later reviewer can audit what was
built without re-reading the whole thread. This is not a changelog and not a
place for marketing.

## Protocol

1. Every work package gets one file: `docs/reviews/YYYY-MM-DD-<slug>.md`.
   - `YYYY-MM-DD` is the day the package was completed (or last materially
     changed, for a placeholder).
   - `<slug>` is lowercase kebab-case, e.g. `scaffold`, `core-store`,
     `release-audit`.
2. The file must contain these fields:
   - **Agent / model** — who did the work and which model.
   - **Commit(s)** — full or short hashes. If the commit does not exist yet,
     write `TBD` and say what the latest local commit is.
   - **Files** — the paths or path groups the package touched.
   - **What changed** — what was built or changed, in plain terms.
   - **Commands + results** — exact command, exit status and the meaningful
     output. Include failures and warnings. Do not paste secrets, tokens,
     private keys or passwords.
   - **Known risks** — what is not verified, what could break, what was
     deliberately deferred.
   - **Review checklist** — concrete questions/checks for the reviewer.
   - **Opus verdict** — empty until Opus reviews. The implementation agent must
     not fill it in.
3. Write facts only. A check is `Pass` only with evidence (a command result or
   a cited file/line). Otherwise mark it `Unverified` or `Fail`.
4. A package may open its file as a placeholder before the commit lands. Set
   `Status: in progress`, keep the commit as `TBD`, and do not ask for review
   yet. Fill the record and change the status when the commit is final.
5. An audit of an existing package may live as a dated section in that
   package's record instead of a new file; the release audit in the scaffold
   record is the first example.
6. If work is amended, append a dated note; do not rewrite history in the file.

## DeepSeek work

Work done by DeepSeek is committed with a `[deepseek]` subject prefix, for
example:

```
[deepseek] Add review ledger and audit release pipeline
```

DeepSeek writes the record except the verdict, and lists the package in the
index below. Opus reviews the commit(s) later and writes only the
`Opus verdict` section (and may append review findings).

The subject prefix starts with this ledger. `924aae1` predates the rule and
does not carry it; the record says so.

## Opus verdict

Use one of:

- `accept`
- `accept with follow-ups` (list them)
- `changes required` (list the blocking items)

Then state the commit reviewed and any follow-up package that should be opened.

## Index

| Date | Work package | Commit | Agent | Status |
| --- | --- | --- | --- | --- |
| 2026-11-17 | [Scaffold: Tauri shell, core store, release pipeline](2026-11-17-scaffold.md) | `924aae1` | DeepSeek implementation sub-agent | Approved with notes |
| 2026-11-17 | [Core store, claims, changes, proposals](2026-11-17-core-store.md) | `c4721f9` | DeepSeek implementation sub-agent | Approved with notes |
| 2026-11-17 | [Snapshot + IPC adapter (main window)](2026-11-17-snapshot-ipc.md) | `91e4f0a` | DeepSeek implementation sub-agent | Approved with notes |
| 2026-11-17 | Format hover.rs (mechanical; no behavior change) | `674c2b9` | DeepSeek implementation sub-agent | Mechanical; Opus may skip |
| 2026-11-17 | [Review ledger + release audit](2026-11-17-scaffold.md#release-audit--2026-11-17) | `70f8eae` | DeepSeek implementation sub-agent | Approved with notes |
| 2026-11-17 | [JEV client + routing](2026-11-17-jev-client.md) | `6b2bad9` | DeepSeek implementation sub-agent | Approved with notes |
| 2026-11-17 | [Core regression tests + claim busy handling](2026-11-17-core-regressions.md) | `70be4a4` | DeepSeek implementation sub-agent | Approved |
| 2026-11-17 | [Release hardening](2026-11-17-release-hardening.md) | `9b9492d` | DeepSeek implementation sub-agent | Changes needed |
| 2026-11-17 | [Main window UI + hover entry](2026-11-17-ui.md) | `c2a5df3` | Opus 5.5 | Self-reviewed; user review pending |
| 2026-11-17 | [Agent MCP endpoint + analyst boundary](2026-11-17-mcp-agent.md) | `3eccbeb` | Opus 5.5 | Self-reviewed; user review pending |
| 2026-11-17 | [MCP setup in Settings](2026-11-17-mcp-ui.md) | `3e3042c` | Opus 5.5 | Self-reviewed; user review pending |
| 2026-11-17 | [Release pipeline + updater](2026-11-17-release-pipeline.md) | `862536a` / v0.1.1 | Opus 5.5 | Validated on a real install |

The release audit is a section of the scaffold record because it audits the
scaffold's release pipeline; it does not change the scaffold commit.
