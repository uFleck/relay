import { describe, expect, it } from "vitest";
import { placeIn, snap } from "./place";

// A 150 % monitor to the right of a 100 % one: work area 2560×1392 physical, starting at x = 1920.
const area = { x: 1920, y: 0, width: 2560, height: 1392 };

describe("placeIn", () => {
  it("puts the tab flush with the right edge at its anchor", () => {
    expect(placeIn(area, { edge: "right", offset: 600 }, 78, 96)).toEqual({ x: 1920 + 2560 - 78, y: 600, width: 78, height: 96 });
  });

  it("puts the tab flush with the left edge", () => {
    expect(placeIn(area, { edge: "left", offset: 600 }, 78, 96).x).toBe(1920);
  });

  it("pulls a panel that would run past the bottom back inside the work area", () => {
    const tabAtBottom = { edge: "right" as const, offset: 1392 - 96 };
    expect(placeIn(area, tabAtBottom, 600, 810).y).toBe(1392 - 810);
    // Collapsing keeps the anchor, so the tab goes back to where it was.
    expect(placeIn(area, tabAtBottom, 78, 96).y).toBe(1392 - 96);
  });

  it("never goes above the work area", () => {
    expect(placeIn({ ...area, y: 40 }, { edge: "right", offset: -50 }, 78, 96).y).toBe(40);
  });
});

describe("snap", () => {
  it("snaps to the nearer edge and keeps the dropped height", () => {
    expect(snap(area, { x: 2000, y: 300, width: 600, height: 810 })).toEqual({ edge: "left", offset: 300 });
    expect(snap(area, { x: 4000, y: 300, width: 78, height: 96 })).toEqual({ edge: "right", offset: 300 });
  });

  it("clamps a drop below the work area", () => {
    expect(snap(area, { x: 4000, y: 1500, width: 78, height: 96 }).offset).toBe(1392 - 96);
  });
});
