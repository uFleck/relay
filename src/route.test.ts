import { describe, expect, it } from "vitest";
import { episodeRoute, formatRoute, parseRoute, proposalRoute, taskRoute } from "./route";

describe("routes", () => {
  it("reads the routes Rust sends from the tray", () => {
    expect(parseRoute("/review").view).toBe("review");
    expect(parseRoute("/settings").view).toBe("settings");
  });

  it("reads a task route with its focus", () => {
    expect(parseRoute("/today?task=01J&focus=changes")).toEqual({ view: "today", task: "01J", focus: "changes", proposal: null, episode: null });
  });

  it("falls back to Today for an unknown path", () => {
    expect(parseRoute("/nope").view).toBe("today");
    expect(parseRoute("").view).toBe("today");
  });

  it("round-trips the hover's links", () => {
    expect(parseRoute(taskRoute("t1"))).toMatchObject({ view: "today", task: "t1", focus: "changes" });
    expect(parseRoute(proposalRoute("p1"))).toMatchObject({ view: "review", proposal: "p1" });
    expect(formatRoute({ view: "agents", task: null, focus: null, proposal: null, episode: null })).toBe("/agents");
  });

  it("reads an episode transcript route", () => {
    expect(episodeRoute("e1")).toBe("/inbox?episode=e1");
    expect(parseRoute("/inbox?episode=e1")).toMatchObject({ view: "inbox", episode: "e1", task: null });
  });
});
