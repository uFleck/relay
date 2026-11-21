import { describe, expect, it } from "vitest";
import { addedKeys } from "./motion";

describe("arrival keys", () => {
  it("marks only genuinely new identities", () => {
    const before = new Set(["note-a", "task-1"]);
    expect(addedKeys(before, ["note-a", "task-1"]).size).toBe(0);
    expect([...addedKeys(before, ["note-a", "task-1", "note-b"]) ]).toEqual(["note-b"]);
  });

  it("does not replay on an unchanged refresh or on reordering", () => {
    const before = new Set(["a", "b"]);
    expect(addedKeys(before, ["b", "a"]).size).toBe(0);
  });

  it("treats a removed id reappearing as an arrival", () => {
    expect([...addedKeys(new Set(["a"]), ["a", "b", "c"])]).toEqual(["b", "c"]);
  });
});
