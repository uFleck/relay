import { describe, expect, it } from "vitest";
import { DOCK_PREFERRED_WIDTH, dockCssHeight, dockLayoutState, SUMMON_CHORD } from "./dock";

describe("Dock hand-off", () => {
  it("exposes exactly the three layout states", () => {
    expect(dockLayoutState(false, false)).toBe("compact");
    expect(dockLayoutState(false, true)).toBe("compact");
    expect(dockLayoutState(true, false)).toBe("picker");
    expect(dockLayoutState(true, true)).toBe("picker-lens");
  });

  it("keeps the capture-bar width and the registered summon chord", () => {
    expect(DOCK_PREFERRED_WIDTH).toBe(640);
    expect(SUMMON_CHORD).toBe("Ctrl+Alt+R");
  });

  it("adds the settled picker and its gap only when the picker is open", () => {
    expect(dockCssHeight(210.2, 280, "compact")).toBe(211);
    expect(dockCssHeight(210.2, 280, "picker")).toBe(497);
    expect(dockCssHeight(210.2, 280, "picker-lens")).toBe(497);
    // A tall picker is capped instead of pushing the top edge off the work area.
    expect(dockCssHeight(210, 900, "picker")).toBe(536);
  });
});
