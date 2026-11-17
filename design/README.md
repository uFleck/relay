# Relay design mockups

Two static mockups, opened straight from disk. They need no network: fonts fall back to Segoe UI / Consolas when Schibsted Grotesk or JetBrains Mono is not installed, and the icons are inline SVG symbols in Phosphor's line style.

- `main.html`: the main window at 1280×860, Today view, with the task sheet open.
- `hover.html`: the always-on-top companion. It shows the edge tab, the idle panel, the needs-you panel, the update-available panel and a live demo (hover the tab; "Simulate change" makes it pulse; "Pin open" holds it open).

Both pages have a day/night toggle. The tokens are Hopper's Circuit theme (`hopper/src/styles/tokens.css` and `tokens-light.css`).

## Decisions after design review

- **Radius is 8 px**, matching Hopper's Circuit tokens. PLAN §5 was corrected.
- **The hover does not approve proposals.** Its action is `Review`, which opens the main window on the proposal. Approve/Reject stay in the main window, so a stray click in an always-on-top window cannot change tasks.
- **Counts share one Today scope.** The hover and the main header must show the same numbers; the mockups' differing counts were placeholder copy.
- **Grouped rows drop the status pill** because the group header already carries the status. The reclaimed width goes to the title; a compact category dot stays.

## Main window

| Region | Size | Notes |
| --- | --- | --- |
| Title bar | 36 px | Custom, drag region, `Ctrl K` search centred, Windows caption buttons (46 px each, close turns `#c42b1c`) |
| Rail | 184 px | Mark plus version, the nav (32 px items, 8 px radius), sync dots above Settings |
| Main column | `minmax(0,1fr)`, about 716 px | Header, grouped rows, review card, agent strip (40 px) |
| Side sheet | 380 px | Fixed header, scrolling body, footer 50 px |

**Rows** are 34 px. The grid is `14 | 1fr | 74 | 104 | 108 | 74 | 46`: check, title, project, status pill, claim, due, links. The title is the only flexible column and truncates with an ellipsis. The full title goes in a `title` tooltip. A row with an unacked change gets a 6 px amber dot after its title. The selected row uses `--sel-bg` with a 2 px accent inset on the left.

**Groups** follow the project's status order. A group header is an 11 px mono uppercase label with a category dot and a count. Empty groups are hidden.

**Colours come from the status category, not the status name**, so a rename keeps its colour:

| Category | Token |
| --- | --- |
| active | `--accent` |
| review | `--warn` |
| merged | `--i4` |
| Waiting for deploy (merged → deploying) | `--i3` |
| deploying | `--s3` |
| blocked | `--err` |
| done | `--ok` |
| todo/backlog | `--text-tertiary` |

**Side sheet.** It is not a route. It opens beside the list, and `Esc` closes it.

- The stepper shows the default workflow without Backlog. Steps before the current one are filled. The current step has a 4 px halo.
- The claim banner is green while the lease is live. It turns amber when the last progress is more than 5 minutes old, and grey after the claim is released.
- The changes timeline puts the newest entry first. An unacked requirement has an amber node with a halo, an amber quote bar, `ack` and `permalink` actions, and the confidence score. An acked entry shows `acked HH:MM · by clm_…`. A field change shows on one line.
- Links sit in a 2-column grid, each with a state on the right.
- The footer holds `Open thread` (primary) and `View diff`.

**Review card.** Only the top proposal from the queue shows on Today, and the full queue lives in Review. The card has:

- the conversation, with the excerpt JEV matched highlighted in `--glow` at 22 %;
- JEV bars, with a tick at each per-project threshold (0.70, 0.80, 0.60);
- a field diff;
- Approve, Edit and Reject, with `A`, `E` and `R` as shortcuts.

**Agent strip.** One pill per live claim, showing the holder, the elapsed time, the task and the time since the last progress. The dot beats while progress is fresh and turns amber when it goes stale.

## Hover companion

| State | Size | Content |
| --- | --- | --- |
| Edge tab | 52×64, 26 px visible | Mark and the Today count. A badge shows the needs-you count (amber) or `↑` for an update (accent) |
| Expanded | 400×540 | Header (mark, needs-you chip, DND, pin, grip), gauge (counts and sync dots), body, update row, footer |
| Update variant | 400 wide, height fits content | Accent row: `0.4.2 ready · 11.8 MB · signed`, Restart |

**Body order changes with the state.** In the idle state it shows active claims, next due, then needs-you ("Nothing waiting"). In the needs-you state, needs-you moves to the top with the proposal card and the unacked change, then active claims follow, and next due drops to the one-line slot above the footer. The panel never scrolls. If there are more than 2 proposals, it shows `+N more in Review`.

**Sync dots.** Green means ok. Amber blinks (1.6 s, `steps(2)`) while the source is retrying or reconnecting. Red means the source is down, which is not shown in the mockup. The tooltip gives the reason, for example "JEV retrying 2/5".

### Interactions

- **Hover to expand.** Rust polls the pointer about every 50 ms, because the webview misreports hover after focus moves (PLAN §5). The window grows the tab into the panel over 260 ms with `--ease`, and the content fades in 120 ms later so text never reflows during the grow. Leaving the panel collapses it after a 400 ms grace period. "Pin open" turns the collapse off.
- **Click a task, claim or change** to open the main window on that task with the change timeline scrolled into view. Click `Open Relay` to open the main window on Today. Approve in the hover applies the proposal and nothing else. It never opens the main window.
- **Pulse on change.** When a new change lands on an active task, or a new proposal arrives, the tab pulses 3 times with an amber ring (2.4 s each), and then the pulse stops. The badge stays until the change is acked or the proposal is decided. Do not disturb stops the pulse but not the badge.
- **Drag to snap.** Drag the header or grip; it uses the OS move loop (`WM_NCLBUTTONDOWN` + `HTCAPTION`). On release, the window snaps to the nearest left or right edge, and its y position is clamped to the work area. The edge and y are saved per monitor. On a left edge the tab mirrors: rounded corners on the right, and the badge in the top-right corner.
- **Focus.** The hover never takes focus. Buttons work through `WS_EX_NOACTIVATE` clicks. Focusing the main window hides the hover unless it is pinned.

## Tokens used

From Circuit: `--bg-page`, `--bg-surface`, `--bg-elevated`, `--border-default`, `--border-strong`, `--text-*`, `--accent*`, `--on-accent`, `--sel-bg`, `--sel-fg`, `--hover-tint`, `--ok`, `--warn`, `--err`, `--i1`–`--i4`, `--s3`, `--glow`, `--shadow-pop`, `--ring`, `--ease`, `--spring`, `--fast` (140 ms) and `--med` (260 ms).

The mockups add these:

| Token | Value |
| --- | --- |
| `--r` | 8 px |
| `--r-sm` | 5 px, for chips and buttons |
| `--r-lg` | 14 px, for the hover panel and tab |
| `--title-h` | 36 |
| `--rail-w` | 184 |
| `--sheet-w` | 380 |
| `--row-h` | 34 |
| `--tab-w` / `--tab-h` | 52 / 64 |
| `--hov-w` / `--hov-h` | 400 / 540 |
| `--c-{active,review,merged,deploying,blocked,done,todo}` | Category colours |

Type: UI text uses `--font-ui` at 13 px, with titles at 21 px (page), 16 px (sheet) and 13 px (hover). Metadata, times, counts, ids and confidence values use `--font-mono` at 10.5–11.5 px.

## React implementation checklist

1. Copy the Circuit `tokens.css` and `tokens-light.css` into `src/styles`. Add the size and category tokens listed above. Bundle Schibsted Grotesk and JetBrains Mono as local `woff2` files.
2. Add a `statusColor(category)` function that is the only map from category to token. Rows, pills, the stepper and group headers all read from it.
3. Build `TaskRow` with the 7-column grid. Use `useSyncExternalStore` selectors per row, so a progress tick re-renders one row, not the list.
4. Build `TaskSheet` as an overlay column driven by `?task=<id>&focus=changes`. The hover's click opens this URL. Close it with `Esc`.
5. Build `ChangeTimeline`. `ack` and `permalink` are inline, and acks are optimistic with a `rev` check.
6. Build `ProposalCard`. Use one component for the Today card, the Review list and the hover card, with `size="full" | "compact"`.
7. Build `ClaimPill` and `ClaimBanner`. Derive `fresh`, `stale` and `released` from `last_progress_at` and the lease. Tick elapsed time from one shared 30 s clock, not a timer per component.
8. Build the hover app as its own Vite entry (`hover.html`). Rust sends `mode: tab | open` and `edge: left | right`. Keep the panel at a fixed 400×540 and resize the window, not the DOM.
9. Make the pulse a CSS class with `animation-iteration-count: 3`. Restart it by removing the class and adding it again on a `change` event.
10. Add a Playwright smoke test for each view: `main` with the sheet open, and the hover's `tab`, `open`, needs-you and update states. Run each at 100 % and 150 % scale.
