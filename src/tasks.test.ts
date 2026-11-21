import { describe, expect, it } from "vitest";
import type { V2Task, V2TaskStatus } from "./ipc";
import {
  groupTasks,
  needsAttention,
  needsYouCount,
  NEEDS_YOU_STATUSES,
  statusTone,
  taskGroup,
  waitingTasks,
  type WorkspaceFilter,
} from "./workspace";

const fixture = (
  id: string,
  status: V2TaskStatus,
  patch: Partial<V2Task> = {},
): V2Task => ({
  id,
  sourceLineId: `line-${id}`,
  noteId: `note-${id}`,
  originalLine: `!${id}`,
  agentMode: "act",
  status,
  projectId: null,
  brief: null,
  grillTurn: null,
  pendingAction: null,
  failureStage: null,
  failureReason: null,
  threadUrl: null,
  references: [],
  createdAt: "2026-11-21T00:00:00Z",
  updatedAt: "2026-11-21T00:00:00Z",
  ...patch,
});

const noFilter: WorkspaceFilter = {
  filtering: false,
  ready: true,
  noteIds: null,
  taskIds: null,
  result: null,
};

const hideSome = (ids: string[]): WorkspaceFilter => ({
  filtering: true,
  ready: true,
  noteIds: null,
  taskIds: new Set(ids),
  result: { query: "notebook filter", noteIds: [], taskIds: ids },
});

describe("task groups", () => {
  const expectedGroups: Record<V2TaskStatus, "needsYou" | "inFlight" | "closed"> = {
    needs_project: "needsYou",
    awaiting_approval: "needsYou",
    needs_answer: "needsYou",
    returned: "needsYou",
    failed: "needsYou",
    briefing: "inFlight",
    grilling: "inFlight",
    dispatched: "inFlight",
    done: "closed",
    cancelled: "closed",
  };

  it("maps every status to one visual tone with coral for needs-you", () => {
    const statuses = Object.keys(expectedGroups) as V2TaskStatus[];
    for (const status of statuses) {
      const group = expectedGroups[status];
      expect(statusTone(status)).toBe(
        group === "needsYou" ? "needs" : group === "inFlight" ? "flight" : "closed",
      );
    }
  });

  it("places every stored status in exactly one group", () => {
    const statuses = Object.keys(expectedGroups) as V2TaskStatus[];
    const tasks = statuses.map((status, i) => fixture(`t${i}`, status));
    const groups = groupTasks(tasks, noFilter);
    expect(groups.needsYou).toHaveLength(5);
    expect(groups.inFlight).toHaveLength(3);
    expect(groups.closed).toHaveLength(2);
    for (const status of statuses) {
      expect(taskGroup(status)).toBe(expectedGroups[status]);
      expect(groups[expectedGroups[status]].some((t) => t.status === status)).toBe(true);
    }
  });

  it("sorts each group by latest change descending with an id tie-breaker", () => {
    const tasks = [
      fixture("01J-older", "needs_project", { updatedAt: "2026-11-20T00:00:00Z" }),
      fixture("01J-b", "failed", { updatedAt: "2026-11-21T00:00:00Z" }),
      fixture("01J-a", "returned", { updatedAt: "2026-11-21T00:00:00Z" }),
      fixture("01J-newer", "needs_project", { updatedAt: "2026-11-22T00:00:00Z" }),
    ];
    const groups = groupTasks(tasks, noFilter);
    expect(groups.needsYou.map((t) => t.id)).toEqual([
      "01J-newer",
      "01J-a",
      "01J-b",
      "01J-older",
    ]);
  });
});

describe("shared attention predicate", () => {
  it("counts the five needs-you statuses", () => {
    for (const status of NEEDS_YOU_STATUSES) {
      expect(needsAttention(fixture(`s-${status}`, status))).toBe(true);
    }
    for (const status of ["briefing", "dispatched", "done", "cancelled"] as V2TaskStatus[]) {
      expect(needsAttention(fixture(`s-${status}`, status))).toBe(false);
    }
  });

  it("keeps an agent-turn Grill out of attention and in In flight", () => {
    const grill = fixture("grill-agent", "grilling", {
      agentMode: "grill",
      grillTurn: "agent",
    });
    expect(needsAttention(grill)).toBe(false);
    expect(taskGroup(grill.status)).toBe("inFlight");
    expect(groupTasks([grill], noFilter).inFlight).toHaveLength(1);
  });

  it("needs the user on a pending question or closing approval", () => {
    for (const grillTurn of ["user_question", "closing_approval"] as const) {
      const grill = fixture(`grill-${grillTurn}`, "grilling", { agentMode: "grill", grillTurn });
      expect(needsAttention(grill)).toBe(true);
      expect(taskGroup(grill.status)).toBe("inFlight");
    }
    // A null turn is the agent's turn, not an implicit need.
    expect(needsAttention(fixture("grill-null", "grilling", { agentMode: "grill" }))).toBe(false);
  });

  it("treats needs answer as a T3 question, not a Grilling turn", () => {
    const question = fixture("t3-question", "needs_answer", { agentMode: "act" });
    expect(needsAttention(question)).toBe(true);
    expect(taskGroup(question.status)).toBe("needsYou");
  });

  it("counts each Task id once even when it appears twice", () => {
    const grill = fixture("grill-once", "grilling", { agentMode: "grill", grillTurn: "user_question" });
    const tasks = [grill, { ...grill }, fixture("other", "failed")];
    expect(waitingTasks(tasks).map((t) => t.id)).toEqual(["grill-once", "other"]);
    expect(needsYouCount(tasks)).toBe(2);
  });

  it("orders the waiting list newest change first", () => {
    const tasks = [
      fixture("old", "failed", { updatedAt: "2026-11-20T00:00:00Z" }),
      fixture("new", "returned", { updatedAt: "2026-11-22T00:00:00Z" }),
      fixture("mid", "needs_project", { updatedAt: "2026-11-21T00:00:00Z" }),
    ];
    expect(waitingTasks(tasks).map((t) => t.id)).toEqual(["new", "mid", "old"]);
  });

  it("does not let a Notebook filter change the count or the Dock list", () => {
    const tasks = [
      fixture("waiting-a", "awaiting_approval", { updatedAt: "2026-11-22T00:00:00Z" }),
      fixture("waiting-b", "grilling", {
        agentMode: "grill",
        grillTurn: "closing_approval",
        updatedAt: "2026-11-21T12:00:00Z",
      }),
      fixture("hidden", "returned", { updatedAt: "2026-11-20T00:00:00Z" }),
      fixture("flight", "dispatched"),
    ];
    const filter = hideSome(["waiting-a", "flight"]); // hides two waiting Tasks, keeps one
    const groups = groupTasks(tasks, filter);
    expect(groups.needsYou.map((t) => t.id)).toEqual(["waiting-a"]);
    expect(groups.inFlight.map((t) => t.id)).toEqual(["flight"]);
    expect(needsYouCount(tasks)).toBe(3);
    expect(waitingTasks(tasks).map((t) => t.id)).toEqual([
      "waiting-a",
      "waiting-b",
      "hidden",
    ]);
  });
});
