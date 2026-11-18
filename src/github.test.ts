import { describe, expect, it } from "vitest";
import { prMeta, prTitle } from "./github";

describe("prMeta", () => {
  it("reads what the poller writes", () => {
    expect(prMeta('{"checks":"failing","review":"changes_requested","headSha":"abc"}')).toEqual({
      checks: "failing",
      review: "changes_requested",
    });
  });

  it("treats missing, empty, broken or unknown metadata as none", () => {
    for (const json of [undefined, "", "{}", "not json", "null", '{"checks":"exploded","review":7}']) {
      expect(prMeta(json)).toEqual({ checks: "none", review: "none" });
    }
  });
});

describe("prTitle", () => {
  it("prefers the synced title and falls back to the short URL", () => {
    const url = "https://github.com/acme/relay/pull/42";
    expect(prTitle({ kind: "pr", label: "Fix the hover", state: "open", url })).toBe("Fix the hover");
    expect(prTitle({ kind: "pr", label: "pr", state: "", url })).toBe("acme/relay/pull/42");
  });
});
