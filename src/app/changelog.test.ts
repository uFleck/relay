import { describe, expect, it } from "vitest";
import { changelogLines } from "./changelog";

describe("changelogLines", () => {
  it("turns GitHub release Markdown into heading, item and text lines", () => {
    const body = [
      "## What's Changed",
      "",
      "* feat: **Search** opens on [Ctrl K](https://example.com) by @someone",
      "- fix: `Dock` sizing",
      "1. numbered item",
      "---",
      "Plain sentence.",
      "<!-- hidden -->",
      "![shot](https://example.com/a.png)",
    ].join("\r\n");

    expect(changelogLines(body)).toEqual([
      { kind: "head", text: "What's Changed" },
      { kind: "item", text: "feat: Search opens on Ctrl K by @someone" },
      { kind: "item", text: "fix: Dock sizing" },
      { kind: "item", text: "numbered item" },
      { kind: "text", text: "Plain sentence." },
    ]);
  });

  it("returns nothing for an empty body", () => {
    expect(changelogLines("  \n\n")).toEqual([]);
  });
});
