# Redesign: Capsule hover, Workspace Projects, Base UI controls

**Status:** final
**Agent / model:** Claude Opus 5.5
**Commit(s):** `7959d33` (controls), `55785ec` (hover), `32c6bb1` (Projects), follow-up fixes after the visual pass
**Files:** `package.json`, `pnpm-lock.yaml`, `src/ui/controls.tsx`, `src/styles/controls.css`, `src/main.tsx`,
`src/app/Import.tsx`, `src/app/Slack.tsx`, `src/app/Views.tsx`, `src/app/Today.tsx`, `src/app/TaskSheet.tsx`,
`src/App.tsx`, `src/styles/app.css`, `src/styles/layout-tokens.css`, `src/hover/HoverApp.tsx`, `src/hover/hover.css`,
`src-tauri/src/hover.rs`

## What changed

The user picked directions from `design/redesign/`: overlay B (Capsule, revision 2), Projects B (Workspace), and Base UI
for controls.

- **Controls.** `@base-ui/react` 1.9.0. `src/ui/controls.tsx` wraps Checkbox, Switch and Select; `controls.css` styles
  them with the Circuit tokens via Base UI's data attributes. ClickUp workspace/space/project, Slack transport and
  the Slack peer "Exclude" use them; Settings → Hover DND and Pause capture are switches. No native `<select>` or
  checkbox is left in `src/`.
- **Hover.** The tab is a flat slab, no mark: needs-you or update badge, today count, a fill for the in-progress share,
  one pip per sync source. The panel: header with round DND / pin / grip, an update banner at the top (available,
  downloading with progress, ready → Restart, check failed → Retry; nothing when up to date), a hero count with
  chips, needs-you cards tinted amber, claims as avatar rows whose ring is the lease left since last progress, next
  due as a pill, and a footer with sync, `Ctrl Alt R` and Open Relay. `--tab-w` 34, `--r-cap` 12; Rust's initial
  `TAB_W`/`TAB_H` follow (the page re-measures and places the window right after).
- **Projects.** One project in focus: a project list with open counts and a status mini-bar, a header with chips,
  a Repository tile (the existing `ProjectRepo`) and an Import sources tile (ClickUp / GitHub, counted from task
  links), and Tasks / Import tabs. `ProjectImport` is deleted; `ImportPanel` is the Import tab. The selected task
  expands under its row: `TaskGroups` takes `renderSelected`, `TaskSheet` takes `inline`, and App does not open the
  side column on Projects. Only the focused project's `ProjectRepo` mounts, so Projects runs one git-state read
  instead of one per project.

## Checks

`pnpm typecheck`, `pnpm test` (14), `pnpm build`, `cargo check` (src-tauri): pass. Main entry after the change: 61.9 KB gz; no baseline measured.

Visual pass in `pnpm dev` (browser, mock data): hover tab and expanded needs-you panel, Projects with a task expanded in place, Settings select open. It found and fixed: Base UI's default `▼` doubling the select chevron, a doubled item check and a horizontal scrollbar in the select popup, and the expanded task scrolling its own row out of view.

## Not covered

- Not run in the Tauri window: window sizing, edge snapping and the left-edge mirror of the new tab are unchecked.
- The note box from the mockup is not built: it needs the notepad backend (separate work item). `hover_focus` already
  exists for the keyboard part.
- Acrylic: the hover window is transparent and sized to the element, so CSS `backdrop-filter` cannot blur the
  desktop. The panel uses the solid surface; a Tauri window effect would be a Rust change.
- The app keeps the Circuit palette and fonts; the mockup's Segoe UI Variable / indigo accent was not adopted.
- A done task selected on Projects has no row (done is hidden), so nothing expands for it.

## Checklist

- [x] No native dialogs added
- [x] No secrets touched
- [x] Policy stays out of adapters (UI only, plus one Rust constant)
- [x] Clicked through in the browser dev build
- [ ] Clicked through in the Tauri window

## Opus verdict

