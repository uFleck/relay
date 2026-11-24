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
| 2026-11-17 | [Slack capture, JEV jobs and reviewer](2026-11-17-slack-jobs.md) | `df5f34a`…`f5ca859` | DeepSeek V4.1 | Changes needed (6 blocking) |
| 2026-11-17 | [Slack and JEV UI](2026-11-17-slack-ui.md) | `e16d3f8` | Opus 5.5 | Self-reviewed; user review pending |
| 2026-11-17 | [Git/PR context and imports](2026-11-17-git-pr-imports.md) | `fff648c`…`9064ec9` | DeepSeek V4.1 | Changes needed (2 blocking) |
| 2026-11-17 | [Final 1.0 UI details](2026-11-17-final-ui.md) | `4db653c` | Opus 5.5 | Self-reviewed; user review pending |
| 2026-11-17 | [On-demand Slack sync](2026-11-17-sync-now.md) | `ed38883` | DeepSeek V4.1 | Awaiting Opus review |
| 2026-11-21 | [v2 Settings, provider recovery and T3 mapping](2026-11-21-v2-settings.md) | `63ff83f5b046213de139142de3e54b3bf86f5a24` | T3 Code / hy4-flash-free | Local implementation complete; awaiting Opus review |
| 2026-11-21 | [v2 prerequisite integration (#27 + #28)](2026-11-21-v2-prerequisite-integration.md) | `a50650005edda62a5a93d780d5c3e0293b616a78` | T3 Code / hy4-flash-free | Local implementation complete; awaiting Opus review |
| 2026-11-21 | [Local Reference picker, Lens and URL chips](2026-11-21-local-references.md) | `6f412bb3f33cb98a0061ec15435a2a51e10ad0e3` | T3 Code / hy4-flash-free | Local implementation complete; live UI deferred; awaiting Opus review |
| 2026-11-21 | [Live work-provider reads and persistent Reference recovery](2026-11-21-provider-references.md) | `a22f5c1dc9d32c74a070226e0895857bc93cc461` | T3 Code / hy4-flash-free | Local implementation complete; live UI deferred; awaiting Opus review |
| 2026-11-21 | [Create-only Note Import](2026-11-21-create-only-note-import.md) | `7546788d07b75adbfe381b0a117b2d0c2997f27f` | T3 Code / hy4-flash-free | Local implementation complete; live UI deferred; awaiting Opus review |
| 2026-11-21 | [Slack permalink thread resolver](2026-11-21-slack-thread-resolver.md) | `05511e6286df8eefaffc3892a9fe22b720616f91` | T3 Code / hy4-flash-free | Local implementation complete; live connector smoke deferred; awaiting Opus review |
| 2026-11-21 | [Slack directory, frozen windows and explicit search](2026-11-21-slack-directory-windows-search.md) | `ceea936254885749412e03b0b888db16630ff04a` | T3 Code / hy4-flash-free | Local implementation complete; live connector smoke deferred; awaiting Opus review |
| 2026-11-21 | [Act and Wait Briefing with isolated OpenCode](2026-11-21-act-wait-briefing.md) | `c944abfc9a550c8223e49f881ec18cf62198bb93` | T3 Code / hy4-flash-free | Local implementation complete; live desktop smoke deferred; awaiting Opus review |
| 2026-11-21 | [Grilling exchange and one-tool endpoint](2026-11-21-grilling-exchange.md) | `fea92b4de697708ddb5a71ee921d08c55bbc226c` | T3 Code / hy4-flash-free | Local implementation complete; live desktop smoke and explicit permission-model review deferred; awaiting Opus review |
| 2026-11-21 | [Start and reconcile local T3 threads](2026-11-21-t3-dispatch.md) | `0e143583c079cfe3c60dc8e781d0a93561fd5802` | T3 Code / hy4-flash-free | Local implementation complete; real-CLI lost-response smoke passed; live desktop smoke deferred; awaiting Opus review |
| 2026-11-21 | [Track progress and continue the same thread](2026-11-21-t3-progress.md) | `ad72d61d9387e428bf37b04a82a6d64bd975072a` | T3 Code / hy4-flash-free | Local implementation complete; real-CLI show/follow-up smoke passed; live desktop, native question, Stop and linked-PR checks deferred; awaiting Opus review |
| 2026-11-21 | [Legacy MCP and task coordination cleanup (#41)](2026-11-21-legacy-mcp-cleanup.md) | `4207f8eae8fb7bf26fd00a38b1c54c3b1917ded3`, `24443157ada2586abef2228abe6ccc2acee71014` | T3 Code / hy4-flash-free | Local implementation complete; legacy routes and reviewer callers removed; live desktop/CLI cases deferred; auth removal routed to maintainer review; awaiting Opus review |
| 2026-11-21 | [Dock + Lens styling](2026-11-21-dock-lens-styling.md) | `b2717fcaf647e01621de7537e3acf5332a131adf` | T3 Code / hy4-flash-free | Local implementation complete; provider/native acceptance deferred; awaiting Opus review |
| 2026-11-21 | [Windows Dock summon, focus and resize](2026-11-21-windows-dock.md) | `ff4a8d5` | T3 Code / hy4-flash-free | Local implementation complete; isolated desktop smoke passed; elevated/full-screen/mixed-DPI deferred; awaiting Opus review |
| 2026-11-21 | [v1 automatic intake removal (#40)](2026-11-21-v1-intake-removal.md) | `fc10c5281365c3de4f1021ccdde30dea74e74302`, `b902bfe00764de4629a33a396bd07a669152927a` | OpenCode / hy4-flash-free | Local implementation complete; live desktop and provider cases deferred; awaiting Opus review |
| 2026-11-21 | [v2 public docs and examples (#42)](2026-11-21-v2-docs-cleanup.md) | final local HEAD | OpenCode / hy4-flash-free | Local implementation complete; awaiting Opus review |
| 2026-11-21 | [v2 packaged Windows cutover (#43)](2026-11-21-v2-release-cutover.md) | `440eb3d`, `d6f4bc2` | OpenCode / hy4-flash-free | v0.9.0 published; live install/T3/provider cases deferred; awaiting Opus review |
| 2026-11-23 | [v2 UI parity and broken-behaviour pass](2026-11-23-v2-ui-parity.md) | final local HEAD | OpenCode / hy4-flash-free | Local implementation complete; native summon/provider cases deferred; awaiting review |
| 2026-11-23 | [Startup panic after the UI parity release](2026-11-23-startup-state-panic.md) | final local HEAD | OpenCode / hy4-flash-free | Root cause found and fixed; smoke check red then green; pending v0.10.1 release |
| 2026-11-24 | [Dock + Lens visual parity follow-up](2026-11-24-dock-lens-parity-followup.md) | `a6bc073`, `5c3b585`, `3100022`, `f4933d9` | OpenCode / restricted free model | v0.10.2 published; composer states exact; 72 rounding pixels remain; native/provider cases deferred |

The release audit is a section of the scaffold record because it audits the
scaffold's release pipeline; it does not change the scaffold commit.
