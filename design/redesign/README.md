# Relay redesign: mockups for a decision

Static, self-contained HTML. Open any file from disk; nothing loads from the network. Every page has a theme toggle (auto / light / dark) and follows `prefers-color-scheme` on auto. Fonts are the ones Windows already ships (Segoe UI Variable, Cascadia Mono, Bahnschrift), so the redesign bundles no web fonts.

| File | Screen | Direction |
| --- | --- | --- |
| `overlay-a-readout.html` | Hover overlay | A · Readout |
| `overlay-b-capsule.html` | Hover overlay | B · Capsule |
| `overlay-c-signal.html` | Hover overlay | C · Signal |
| `projects-a-ledger.html` | Projects | A · Ledger |
| `projects-b-workspace.html` | Projects | B · Workspace |
| `projects-c-board.html` | Projects | C · Board |
| `controls-kit.html` | Controls | Kit comparison + samples at 100 % / 150 % |

Each overlay page shows the edge tab (calm, needs you, update), the expanded idle panel, the expanded needs-you panel, the four update-row states, and a live dock: hover to expand, 400 ms collapse grace, Pin holds it open, DND stops the pulse, plus "Simulate change", state and update cycling. Each Projects page shows a project header with repo and import controls, task groups, an empty project (Portfolio), and a selected task. Rows, tabs and import toggles are clickable.

## Overlay

| | A · Readout | B · Capsule | C · Signal |
| --- | --- | --- | --- |
| Idea | Instrument panel: hairline grid, mono numerals | Windows 11 acrylic companion | Transit board: status-colour strip |
| Tab | 30 px column: count, amber needs block, sync ticks | Half capsule, done/today ring, badge | 24 px strip: count cap + category stack sized by count |
| Lease | 15 segments, one per minute | Ring around the agent avatar | Text + colour edge by category |
| Needs you | Amber chip + left-ruled rows | Amber-tinted cards, avatar rows | Full-width amber header band |
| Update | Row above footer | Banner at the top | Accent row above footer |
| Density | Highest | Lowest (≈ 15 % taller) | High |
| Risk | Can read as "dev tool" | Acrylic `backdrop-filter` costs GPU in an always-on-top window; needs a solid fallback | Colour stack needs a legend the first time |

## Projects

| | A · Ledger | B · Workspace | C · Board |
| --- | --- | --- | --- |
| Layout | All projects, one scroll, sticky headers | Project list + one project in focus | Project tabs + workflow lanes |
| Repo / import | One toolbar per project, sources inline | Two tiles (Repository, Import sources) + Import tab | Command bar; import drawer |
| Tasks | 32 px rows, status rings | 34 px rows, avatars, in-place expansion | Two-line cards per lane |
| Selected task | 380 px side sheet (same as Today) | Expands in place | Inspector docked at the bottom |
| Empty state | Inline dashed row with actions | Three-step "fill this project" card | Sign + two actions |
| Best when | Few projects, scanning everything | Many projects, deep work in one | Workflow position matters most |
| Cost | Lowest; reuses `TaskGroups` and the sheet | Medium; new master list + expansion | Highest; new lane layout, needs ≥ 1200 px |

## Controls: recommendation

**Base UI (`@base-ui/react`, v1) under a small `src/ui/` layer, styled with Relay tokens in plain CSS.**

| Kit | Kind | Bundle (est.) | Theming with Relay tokens | A11y | Migration |
| --- | --- | --- | --- | --- | --- |
| **Base UI** | Headless | ~20–35 KB gz | Data attributes + CSS vars | Strong | Low |
| Radix Primitives | Headless | ~25–40 KB gz | Same model | Strong | Low; maintenance slowing |
| React Aria Components | Headless | ~45–70 KB gz | Render props + data attributes | Best | Medium |
| shadcn/ui | Copied code over Radix/Base UI | Primitive + Tailwind | Requires Tailwind | Inherits | Medium |
| Mantine | Styled | ~80–120 KB gz + CSS | Second token system | Good | Medium |
| Chakra v3 | Styled | ~100 KB+ gz | Own tokens + Emotion runtime | Good | High |
| MUI | Styled | ~100–140 KB gz | Material look | Good | High |

Why Base UI: Relay already has plain-CSS tokens and wants its own look, so a styled kit would mean fighting a second theme. Base UI has no runtime styling, ships as one tree-shaken package, supports React 19, and positions its popups with Floating UI inside the WebView2 window. It also provides a Dialog for the in-app confirm that `CLAUDE.md` requires. React Aria is the fallback if a11y or i18n depth becomes the priority.

Bundle sizes are estimates, not measurements: this task allowed no installs. Measure them with `vite build` and a visualizer before committing to a kit. Confirm the package name and version at install time.

Migration sketch (not done here): add `src/ui/{Checkbox,Radio,Switch,Select,Button,Input,Dialog,Tooltip}.tsx` with CSS in `src/styles/controls.css`, then replace the native `<input type="checkbox">` and `<select>` in `Import.tsx` and the settings forms, and the `.btn` / `.inp` classes.

## My pick

- **Overlay: A · Readout.** It is the densest of the three and the cheapest to render (no blur), and the tab keeps the most information in the least width. Borrow B's agent avatars if identity matters more than raw text.
- **Projects: A · Ledger.** It keeps Relay's density and the existing side sheet, so the work stays a restyle plus a toolbar instead of a new layout. Take B's empty state and its repo tile copy ("↑2 to push", "uncommitted").
- **Controls: Base UI + Relay tokens**, with the token switch in `controls-kit.html` set to the chosen direction.

Directions can be mixed per screen, but the overlay and Projects should share one token set (radius, accent, type), so pick those two from the same column or accept the token set of whichever overlay you pick.
