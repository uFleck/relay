import { describe, expect, it } from "vitest";
import { draftLabel, draftSummary, lineDirective } from "./directives";

describe("draft directives", () => {
  it("matches the Rust parse: ?? before ? before !, only at column one", () => {
    expect(lineDirective("?? Should exports run in a queue?")).toBe("grill");
    expect(lineDirective("? Check the release notes")).toBe("wait");
    expect(lineDirective("! Review the export log")).toBe("act");
    expect(lineDirective("!no space needed")).toBe("act");
    expect(lineDirective(" ! leading space is plain")).toBeNull();
    expect(lineDirective("Review the export log")).toBeNull();
  });

  it("counts each directive and keeps the first mode for the wordmark", () => {
    const summary = draftSummary(
      "plain note\n! Review the export log\n? Check the release notes\n?? Should exports run in the background?",
    );
    expect(summary.mode).toBe("act");
    expect(summary.counts).toEqual({ act: 1, wait: 1, grill: 1 });
    expect(draftLabel(summary)).toBe("3 tasks · 1 act · 1 wait · 1 grill");
  });

  it("uses the first directive even when it is not on the first line", () => {
    expect(draftSummary("a plain line\n?? grill first").mode).toBe("grill");
    expect(draftLabel(draftSummary("\n\n! only one"))).toBe("1 task · 1 act");
    expect(draftLabel(draftSummary(""))).toBe("note");
  });
});
