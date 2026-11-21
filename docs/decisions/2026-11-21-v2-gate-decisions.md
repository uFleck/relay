# Relay v2 gate decisions (2026-11-21)

Source: user decisions collected after a `draft-issues docs/SPEC-v2.md` re-run confirmed the spec is already fully tracked by parent [#23](https://github.com/uFleck/relay/issues/23) and children #24–#43. No issues, labels or milestones were changed; this file is the durable record.

## Summary

| Issue | Gate | Decision |
| --- | --- | --- |
| [#35](https://github.com/uFleck/relay/issues/35) | Grilling permission boundary | Approve narrow read-only endpoint |
| [#40](https://github.com/uFleck/relay/issues/40) | v1 Slack/JEV credentials | Delete with the code |
| [#41](https://github.com/uFleck/relay/issues/41) | Legacy MCP routes/tokens | Retire both after #37 |
| [#43](https://github.com/uFleck/relay/issues/43) | v2 release posture | Fresh-start on stable |
| all four | Merge policy | Pre-authorize unattended merge |

## Decisions

### #35 — Grilling agent boundary

Ship a dedicated read-only Relay endpoint that serves Reference content to the bundled Grilling agent, with an isolated OpenCode config and **no repo or filesystem access**. Only the resulting Brief goes to T3. The approved product behavior is already in #10 and #15; this decision authorizes the new permission boundary and its merge.

Alternative declined: local data only (no new endpoint), and a contract-first review pause.

### #40 — v1 credentials

Delete v1 Slack app-token and JEV credential handling along with the code. Fresh start per #21: v1 profile data and stored credentials stay outside the v2 store and nothing is migrated back.

Alternatives declined: keep tokens but disable the paths, and pause removal until the on-demand Slack path has run longer.

### #41 — Legacy MCP

Retire both `/mcp` and `/mcp/analyst`, their tokens and setup surfaces once #37 lands (R4/R5/R8). Keep the Grilling Reference endpoint and Task search, links and events used by v2.

Alternatives declined: keep `/mcp` as a compatibility shim for one release, and hold for manual review of the exact routes/tools.

### #43 — Release posture

Ship v2 as a fresh-start upgrade on the stable channel through the existing release process. Existing installs open an empty Notebook with fresh provider/project setup; no v1 data migration, backup or rollback workflow, and no prerelease suffix — `/releases/latest` moves to v2.

Alternatives declined: prerelease first, and a manual tag/upload gate.

### Merge policy

Pre-authorize unattended merge for #35, #40, #41 and #43: once their checks and review pass, `/ship` may merge without stopping. No further decision is required at those four gates.

## Notes

- The `Blocked by #` chain from #25 to #43 is sequencing, not a decision. #24 has no prerequisites and is unblocked.
- If a gate's scope changes materially from what is written here, bring it back for a new decision rather than proceeding on this record.
