import { describe, expect, it } from "vitest";
import {
  cacheAge,
  chipAtCaret,
  convertUrls,
  findMarks,
  copyText,
  groupRows,
  mergeLiveRows,
  parseUrlValue,
  rowIndexByKey,
  parseUrls,
  restoreChip,
  type EditorState,
  type V2PickerRow,
} from "./references";

describe("canonical URL parsing", () => {
  it("recognises ClickUp, GitHub PR/issue and Slack reply permalinks", () => {
    expect(parseUrlValue("https://app.clickup.com/t/8x1K2")?.identity).toBe("clickup:8x1K2");
    expect(parseUrlValue("https://github.com/Sample/App/pull/142")?.canonical).toBe(
      "https://github.com/sample/app/pull/142",
    );
    expect(parseUrlValue("https://github.com/Sample/App/issues/7")?.identity).toBe(
      "github:sample/app#7",
    );
    const slack = parseUrlValue(
      "https://acme.slack.com/archives/C1/p1731000000000099?thread_ts=1731000000.000001",
    );
    expect(slack?.identity).toBe("slack:C1/1731000000.000001");
    expect(slack?.canonical).toBe(
      "https://acme.slack.com/archives/C1/p1731000000000099?thread_ts=1731000000.000001",
    );
    expect(parseUrlValue("https://example.com/x")).toBeNull();
  });

  it("scans multiline text, trims punctuation and leaves unsupported URLs as text", () => {
    const text =
      "!ship (https://app.clickup.com/t/AB), https://github.com/Sample/App/issues/9\nplain https://example.com/x";
    const { urls } = parseUrls(text);
    expect(urls).toHaveLength(2);
    expect(urls[0].value).toBe("https://app.clickup.com/t/AB");
    expect(urls[1].value).toBe("https://github.com/Sample/App/issues/9");
    expect(urls.every((url) => !url.value.includes(","))).toBe(true);
  });
});

describe("chip editor transforms", () => {
  const initial: EditorState = { text: "", chips: [] };

  it("converts every URL on a multiline paste in one change and preserves text", () => {
    const text = "!ship https://app.clickup.com/t/AB\nsee https://github.com/Sample/App/pull/7 ok";
    const next = convertUrls(initial, text, parseUrls(text).urls);
    expect(next.text).toBe("!ship @CU-AB\nsee @app#7 ok");
    expect(next.text.split("\n")).toHaveLength(2);
    expect(next.chips.map((chip) => chip.key)).toEqual(["clickup:AB", "github:sample/app#7"]);
  });

  it("copies URL-backed chips as canonical links and restores one chip on Backspace", () => {
    const text = "!work https://github.com/Sample/App/pull/7";
    const converted = convertUrls(initial, text, parseUrls(text).urls);
    expect(copyText(converted)).toBe("!work https://github.com/sample/app/pull/7");

    const typed: EditorState = {
      text: "!work @CU-AB ",
      chips: [
        {
          key: "clickup:AB",
          token: "@CU-AB",
          kind: "clickup",
          canonicalUrl: "https://app.clickup.com/t/AB",
          originalUrl: "https://app.clickup.com/t/AB",
        },
      ],
    };
    const chip = chipAtCaret(typed, 12);
    expect(chip?.key).toBe("clickup:AB");
    expect(restoreChip(typed, chip!, " ").text).toBe("!work https://app.clickup.com/t/AB");
  });

  it("keeps chip identity when a title changes", () => {
    const converted = convertUrls(initial, "https://app.clickup.com/t/AB", parseUrls("https://app.clickup.com/t/AB").urls);
    const same = convertUrls(converted, converted.text, []);
    expect(same.chips[0].key).toBe(converted.chips[0].key);
    expect(same.text).toBe(converted.text);
  });

  it("decorates every occurrence of a repeated chip on separate lines", () => {
    const chip: EditorState["chips"][number] = {
      key: "clickup:AB",
      token: "@CU-AB",
      kind: "clickup",
      canonicalUrl: "https://app.clickup.com/t/AB",
      originalUrl: "https://app.clickup.com/t/AB",
    };
    const state: EditorState = { text: "!one @CU-AB\n!two @CU-AB", chips: [chip] };
    const marks = findMarks(state, null);
    expect(marks).toHaveLength(2);
    expect(marks.map((mark) => mark.at)).toEqual([5, 17]);
    expect(marks.every((mark) => mark.kind === "chip")).toBe(true);
  });
});

function row(group: V2PickerRow["group"], key: string): V2PickerRow {
  return {
    key,
    kind: "github",
    displayId: key,
    canonicalLink: "",
    availability: "ready",
    group,
    title: key,
    state: null,
    metadata: [],
    snapshot: null,
    unavailableReason: null,
    settingsDestination: null,
    updatedAt: "",
    exactMatch: false,
  };
}

describe("picker row grouping and Lens age", () => {
  it("omits empty groups and keeps the fixed order", () => {
    const groups = groupRows([
      row("notesTasks", "note:1"),
      row("prs", "github:a/b#1"),
      row("cards", "clickup:1"),
    ]);
    expect(groups.map((section) => section.group)).toEqual(["cards", "prs", "notesTasks"]);
  });

  it("formats a cache age", () => {
    expect(cacheAge(null)).toBeNull();
    expect(cacheAge(new Date(Date.now() - 2 * 60 * 60 * 1000).toISOString())).toBe("2h ago");
  });

  it("refreshes a duplicate in place, appends a new row and keeps the highlighted key", () => {
    const local = [row("prs", "github:a/b#1"), row("notesTasks", "note:9")];
    const refreshed = { ...row("prs", "github:a/b#1"), title: "now titled" };
    const fresh = row("issues", "github:c/d#8");
    const merged = mergeLiveRows(local, [refreshed, fresh]);

    expect(merged[0].title).toBe("now titled");
    expect(merged[1].key).toBe("note:9");
    expect(merged[2].key).toBe("github:c/d#8");
    expect(groupRows(merged).map((section) => section.group)).toEqual([
      "prs",
      "issues",
      "notesTasks",
    ]);
    expect(rowIndexByKey(merged, "note:9")).toBe(1);
    expect(rowIndexByKey(merged, "missing")).toBe(0);
  });

  it("a late duplicate refresh does not duplicate or reorder rows", () => {
    const first = mergeLiveRows([], [row("cards", "clickup:1"), row("cards", "clickup:2")]);
    const late = mergeLiveRows(first, [{ ...row("cards", "clickup:1"), title: "late" }]);
    expect(late).toHaveLength(2);
    expect(late.map((entry) => entry.key)).toEqual(["clickup:1", "clickup:2"]);
    expect(late[0].title).toBe("late");
  });
});
