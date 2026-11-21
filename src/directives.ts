import type { V2AgentMode } from "./ipc";

/* Draft-level view of the sigils the Rust composer will parse on save. This only drives
   decoration: the store still owns the authoritative parse. `??` is matched before `?`
   and `!`, and a sigil is only a directive at column one, as in relay-core::v2::composer. */

export const MODE_SIGIL: Readonly<Record<V2AgentMode, string>> = {
  act: "!",
  wait: "?",
  grill: "??",
};

export type ModeCounts = Readonly<Record<V2AgentMode, number>>;

export interface DraftSummary {
  /** The first directive in the draft, which colours the wordmark dot. */
  mode: V2AgentMode | null;
  counts: ModeCounts;
}

export const EMPTY_DRAFT: DraftSummary = {
  mode: null,
  counts: { act: 0, wait: 0, grill: 0 },
};

export function lineDirective(line: string): V2AgentMode | null {
  if (line.startsWith("??")) return "grill";
  if (line.startsWith("?")) return "wait";
  if (line.startsWith("!")) return "act";
  return null;
}

/** Counts every directive line. Plain lines, including lines with leading space, are Notes. */
export function draftSummary(text: string): DraftSummary {
  const counts = { act: 0, wait: 0, grill: 0 };
  let mode: V2AgentMode | null = null;
  for (const line of text.split("\n")) {
    const directive = lineDirective(line);
    if (!directive) continue;
    if (mode === null) mode = directive;
    counts[directive] += 1;
  }
  return { mode, counts };
}

/** A short footer label derived from the same counts, with no stored state involved. */
export function draftLabel(summary: DraftSummary): string {
  const total = summary.counts.act + summary.counts.wait + summary.counts.grill;
  if (total === 0) return "note";
  const parts: string[] = [];
  if (summary.counts.act) parts.push(`${summary.counts.act} act`);
  if (summary.counts.wait) parts.push(`${summary.counts.wait} wait`);
  if (summary.counts.grill) parts.push(`${summary.counts.grill} grill`);
  return `${total} ${total === 1 ? "task" : "tasks"} · ${parts.join(" · ")}`;
}
