import { describe, expect, it } from "vitest";
import { vimStep, type VimCommand } from "./vim";

/** Feeds a key sequence through the grammar and collects the commands it produces. */
function run(keys: (string | { key: string; ctrl: boolean })[]): VimCommand[] {
  let pending = "";
  const commands: VimCommand[] = [];
  for (const raw of keys) {
    const step = vimStep(pending, typeof raw === "string" ? { key: raw, ctrl: false } : raw);
    pending = step.pending;
    if (step.command) commands.push(step.command);
  }
  return commands;
}

describe("vimStep", () => {
  it("maps single keys", () => {
    expect(run(["j", "k", "G", "Enter", "o", "e", "i", "a", "/", "?"])).toEqual([
      "down", "up", "bottom", "open", "open", "edit", "newNote", "newNote", "search", "help",
    ]);
  });

  it("waits for the second key of a pair", () => {
    expect(run(["g", "g", "g", "t", "g", "n", "g", "s", "d", "d"])).toEqual([
      "top", "goTasks", "goNotebook", "goSettings", "delete",
    ]);
  });

  it("drops an unknown pair instead of running its second key", () => {
    expect(run(["g", "j", "j"])).toEqual(["down"]);
    expect(run(["d", "k"])).toEqual([]);
  });

  it("scrolls half a page with ctrl+d and ctrl+u", () => {
    expect(run([{ key: "d", ctrl: true }, { key: "u", ctrl: true }])).toEqual(["halfDown", "halfUp"]);
  });

  it("leaves other keys and chords to the app", () => {
    expect(vimStep("", { key: "x", ctrl: false }).handled).toBe(false);
    expect(vimStep("", { key: "k", ctrl: true }).handled).toBe(false);
    expect(vimStep("", { key: "g", ctrl: false })).toEqual({ command: null, pending: "g", handled: true });
  });
});
