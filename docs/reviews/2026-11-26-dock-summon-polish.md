# Dock summon and Windows UI polish

- Agent and model. Codex, GPT-6.2 Sol.
- Commit. This change is based on `f91b6cb`, the v0.13.1 release commit.
- Files. The Dock, Tasks, Settings, shared controls, and their styles.

## What changed

- The reduced-motion override created a brief visibility transition on the Dock. Focusing the textarea during that transition left the caret on the document body. Reduced motion now disables transitions, and the Dock focuses its textarea after React makes it visible.
- The Dock rises into view with a short fade and scale animation. The native window measures the untransformed content.
- The task sheet uses the content column at 900 CSS pixels and below. Escape returns focus to the task row, or to the heading when the row is unavailable.
- Input switches and selects in Settings have accessible names. Muted labels and light-mode warnings have stronger contrast.

## Verification

- On installed v0.13.1, Ctrl+Alt+R showed and focused the Dock. Three settled runs returned focus to the previous window after Escape. An initial immediate Escape stalled and required another dismissal.
- In the isolated Windows build, a configured Ctrl+Alt+F9 performed eight summon and Escape cycles in each motion mode. Every cycle focused the textarea and restored the caller window. Native keyboard input also verified draft retention, toggle dismissal, Ctrl+Enter capture, and Ctrl+K search for the saved Note.
- Browser fixtures covered all three main views. The task sheet was checked from 640 to 1280 CSS pixels. Axe reported zero WCAG A or AA violations in the default Notebook, Tasks, and Settings states in light and dark mode.
- `pnpm test` passed all 54 tests. `pnpm build` and `git diff --check` passed.

## Limits

- The old prototype parity check already failed against v0.13.1 in all six image comparisons. Its Dock smoke passed.
- The isolated build used a second shortcut to leave the installed app's default registration available. Full-screen applications and multiple monitors were outside this check.

## Review checklist

- Summon from another window and start typing. Confirm the caret appears with reduced motion on and off.
- Dismiss with Escape and summon again. Confirm the draft persists and the previous window regains focus after dismissal.
- Open a task other than the first row at 640 CSS pixels. Close it and confirm focus on the same row.

## Opus verdict
