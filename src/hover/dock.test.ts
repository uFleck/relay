import { describe, expect, it } from "vitest";
import { DOCK_PREFERRED_WIDTH, dockLayoutState, SUMMON_CHORD } from "./dock";

describe("Dock hand-off", () => {
  it("exposes exactly the three layout states", () => {
    expect(dockLayoutState(false, false)).toBe("compact");
    expect(dockLayoutState(false, true)).toBe("compact");
    expect(dockLayoutState(true, false)).toBe("picker");
    expect(dockLayoutState(true, true)).toBe("picker-lens");
  });

  it("keeps the prototype width and the registered summon chord", () => {
    expect(DOCK_PREFERRED_WIDTH).toBe(760);
    expect(SUMMON_CHORD).toBe("Ctrl+Alt+R");
  });
});
