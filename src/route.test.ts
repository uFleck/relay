import { describe, expect, it } from "vitest";
import { formatRoute, notebookRoute, noteRoute, parseRoute, settingsRoute, taskRoute } from "./route";

describe("routes", () => {
  it("reads the three v2 routes", () => {
    expect(parseRoute("/notebook")).toEqual({ view: "notebook", task: null, note: null, provider: null });
    expect(parseRoute("/tasks?task=01J")).toEqual({ view: "tasks", task: "01J", note: null, provider: null });
    expect(parseRoute("/settings?provider=github")).toEqual({
      view: "settings",
      task: null,
      note: null,
      provider: "github",
    });
  });

  it("sends retired v1 routes to Notebook", () => {
    for (const route of ["/review", "/review?proposal=p1", "/inbox?episode=e1", "/today?task=01J", "/agents", "/nope", ""]) {
      expect(parseRoute(route).view).toBe(route.startsWith("/today") ? "tasks" : "notebook");
    }
  });

  it("keeps a task link from a retired route and drops its old focus", () => {
    expect(parseRoute("/today?task=01J&focus=changes")).toEqual({
      view: "tasks",
      task: "01J",
      note: null,
      provider: null,
    });
  });

  it("opens Settings at its heading for an unknown or missing provider", () => {
    expect(parseRoute("/settings?provider=unknown")).toEqual({
      view: "settings",
      task: null,
      note: null,
      provider: null,
    });
    expect(parseRoute("/settings").provider).toBeNull();
    expect(parseRoute("/notebook?provider=slack").provider).toBeNull();
  });

  it("round-trips the helpers used by the tray, hover and sheets", () => {
    expect(taskRoute("t1")).toBe("/tasks?task=t1");
    expect(noteRoute("n1")).toBe("/notebook?note=n1");
    expect(settingsRoute("slack")).toBe("/settings?provider=slack");
    expect(settingsRoute()).toBe("/settings");
    expect(notebookRoute()).toBe("/notebook");
    expect(parseRoute(taskRoute("t1")).task).toBe("t1");
    expect(parseRoute(noteRoute("n1")).note).toBe("n1");
    expect(formatRoute({ view: "notebook", task: null, note: null, provider: null })).toBe("/notebook");
  });

  it("treats an empty selected id as no selection", () => {
    expect(parseRoute("/tasks?task=").task).toBeNull();
    expect(parseRoute("/notebook?note=").note).toBeNull();
  });
});
