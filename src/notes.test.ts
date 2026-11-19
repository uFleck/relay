import { describe, expect, it } from "vitest";
import type { Note } from "./ipc";
import { composePrompt, noteLabel, sortNotes } from "./notes";

const note = (id: string, pinned: boolean, updatedAt: string): Note => ({ id, title: id, body: "", pinned, createdAt: updatedAt, updatedAt });

describe("composePrompt", () => {
  it("leads with the template and the title, then the body", () => {
    expect(composePrompt({ title: " Notes view ", body: "list + editor\n" }, "/ship")).toBe("/ship Notes view\n\nlist + editor");
  });

  it("drops the parts that are empty", () => {
    expect(composePrompt({ title: "", body: "just the body" }, "")).toBe("just the body");
    expect(composePrompt({ title: "Only a title", body: "  " }, "")).toBe("Only a title");
    expect(composePrompt({ title: "", body: "body" }, "/orchestrate")).toBe("/orchestrate\n\nbody");
  });
});

describe("noteLabel", () => {
  it("falls back to the first body line, then Untitled", () => {
    expect(noteLabel({ title: "", body: "\n  first line\nsecond" })).toBe("first line");
    expect(noteLabel({ title: " ", body: " " })).toBe("Untitled");
  });
});

describe("sortNotes", () => {
  it("puts pinned notes first, then the most recent", () => {
    const sorted = sortNotes([note("old", false, "2026-01-01"), note("new", false, "2026-02-01"), note("pin", true, "2025-01-01")]);
    expect(sorted.map((n) => n.id)).toEqual(["pin", "new", "old"]);
  });
});
