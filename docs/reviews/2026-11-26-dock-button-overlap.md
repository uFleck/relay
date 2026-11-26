# Dock button overlap

Reviewed by Codex (GPT-6.2 Sol). Base commit `4e49c9c`.

The Capture button overlapped the shortcut footer in the installed Windows Dock. The flex wrapper stretched the measured content to the current WebView height, which compressed the composer and gave the native window an undersized height. The wrapper clips overflow while letting the measured content keep its natural height.

Files reviewed are `src/styles/dock.css` and `scripts/visual-parity.mjs`.

- [x] The Dock smoke check in `pnpm visual:parity` reported overlapping controls before the change and separated controls after it. All six unrelated prototype comparisons retained their previous results.
- [x] A Chromium check covered a constrained 119 px viewport at widths of 640, 400, 320, and 240 px. The footer stayed below Capture, and both fitted inside the viewport once resized.
- [x] An isolated Windows WebView reached the full 148 px height on eight native shortcut summons, four per motion setting. Both controls passed hit testing. Clicking Capture saved and dismissed the Dock. Clicking Open Relay activated the main window.
- [x] All 54 tests passed in `pnpm test`. `pnpm build` and `git diff --check` passed.

Native testing used one display at 100% scaling.

## Opus verdict
