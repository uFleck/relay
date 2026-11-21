import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  errorText,
  onV2Change,
  v2WorkspaceSearch,
  v2WorkspaceSnapshot,
  type V2Note,
  type V2Task,
  type V2TaskStatus,
  type V2WorkspaceSnapshot,
  type WorkspaceSearchResult,
} from "./ipc";

/* Shared workspace read, filter path and timeline helpers. Notebook and Tasks consume the
   same snapshot, the same search result and the same post-commit refresh signal. */

export const SIGIL: Record<V2Note["tasks"][number]["agentMode"], string> = {
  act: "!",
  wait: "?",
  grill: "??",
};

const pad = (n: number) => String(n).padStart(2, "0");

/** The local calendar day a Note was created on, as `YYYY-MM-DD`. */
export function dayKey(iso: string) {
  const d = new Date(iso);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export const todayKey = () => dayKey(new Date().toISOString());

export function dayLabel(key: string) {
  const today = todayKey();
  if (key === today) return "Today";
  const yesterday = new Date();
  yesterday.setDate(yesterday.getDate() - 1);
  if (key === dayKey(yesterday.toISOString())) return "Yesterday";
  const [year, month, day] = key.split("-").map(Number);
  return new Date(year, month - 1, day).toLocaleDateString("en-GB", {
    weekday: "short",
    day: "numeric",
    month: "short",
    year: "numeric",
  });
}

/** Oldest first, with the immutable ULID as the tie-breaker for equal timestamps. */
export function sortTimeline(notes: readonly V2Note[]): V2Note[] {
  return [...notes].sort(
    (a, b) => Date.parse(a.createdAt) - Date.parse(b.createdAt) || a.id.localeCompare(b.id),
  );
}

export interface DayGroup {
  key: string;
  label: string;
  notes: V2Note[];
}

/** Groups by local creation day. Today is always present, even with no Notes, as an anchor. */
export function groupByDay(notes: readonly V2Note[]): DayGroup[] {
  const byDay = new Map<string, V2Note[]>();
  for (const note of sortTimeline(notes)) {
    const key = dayKey(note.createdAt);
    const group = byDay.get(key);
    if (group) group.push(note);
    else byDay.set(key, [note]);
  }
  const today = todayKey();
  if (!byDay.has(today)) byDay.set(today, []);
  return [...byDay.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([key, group]) => ({ key, label: dayLabel(key), notes: group }));
}

/** True when the reader is at the newest end of a scroll area. */
export function nearBottom(element: HTMLElement | null, threshold = 56) {
  if (!element) return true;
  return element.scrollHeight - element.scrollTop - element.clientHeight <= threshold;
}

export interface WorkspaceState {
  workspace: V2WorkspaceSnapshot | null;
  /** Increments on every committed-write refresh, so an active filter re-runs. */
  version: number;
  error: string | null;
  refresh: () => Promise<void>;
}

export function useWorkspace(): WorkspaceState {
  const [workspace, setWorkspace] = useState<V2WorkspaceSnapshot | null>(null);
  const [version, setVersion] = useState(0);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setWorkspace(await v2WorkspaceSnapshot());
      setVersion((n) => n + 1);
      setError(null);
    } catch (cause) {
      setError(errorText(cause));
    }
  }, []);

  useEffect(() => {
    void refresh();
    // One shared, cross-window signal: a Dock submission refreshes both views.
    return onV2Change(() => void refresh());
  }, [refresh]);

  return { workspace, version, error, refresh };
}

export interface WorkspaceFilter {
  filtering: boolean;
  ready: boolean;
  noteIds: ReadonlySet<string> | null;
  taskIds: ReadonlySet<string> | null;
  result: WorkspaceSearchResult | null;
}

/** Runs the shared search path and drops a response that no longer matches the query. */
export function useWorkspaceFilter(query: string, version: number): WorkspaceFilter {
  const [result, setResult] = useState<WorkspaceSearchResult | null>(null);
  const sequence = useRef(0);
  const filtering = query.trim().length > 0;

  useEffect(() => {
    const request = ++sequence.current;
    if (!filtering) {
      setResult(null);
      return;
    }
    let gone = false;
    void v2WorkspaceSearch(query)
      .then((answer) => {
        if (!gone && request === sequence.current) setResult(answer);
      })
      .catch(() => {
        if (!gone && request === sequence.current) setResult({ query, noteIds: [], taskIds: [] });
      });
    return () => {
      gone = true;
    };
  }, [query, version, filtering]);

  const current = result !== null && result.query === query ? result : null;
  return {
    filtering,
    ready: !filtering || current !== null,
    noteIds: useMemo(() => (current ? new Set(current.noteIds) : null), [current]),
    taskIds: useMemo(() => (current ? new Set(current.taskIds) : null), [current]),
    result: current,
  };
}

/** A Note is visible when it or one of its Tasks satisfied the whole query. */
export function noteMatches(
  note: V2Note,
  filter: WorkspaceFilter,
): boolean {
  if (!filter.filtering || !filter.noteIds) return true;
  return filter.noteIds.has(note.id);
}

export function taskMatches(
  task: V2Note["tasks"][number],
  filter: WorkspaceFilter,
): boolean {
  if (!filter.filtering || !filter.taskIds) return true;
  return filter.taskIds.has(task.id);
}

/* ── Task grouping and the single needs-you predicate (#27) ──
   Pure functions over the unfiltered workspace snapshot. Both the main window and the
   Dock import the same attention rule, so a filter can never change the badge. */

/** The five stored statuses that wait on the user, independent of Grilling turn. */
export const NEEDS_YOU_STATUSES: ReadonlySet<V2TaskStatus> = new Set<V2TaskStatus>([
  "needs_project",
  "awaiting_approval",
  "needs_answer",
  "returned",
  "failed",
]);

export type TaskGroupId = "needsYou" | "inFlight" | "closed";

const TASK_GROUP: Readonly<Record<V2TaskStatus, TaskGroupId>> = {
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

export function taskGroup(status: V2TaskStatus): TaskGroupId {
  return TASK_GROUP[status];
}

/** Latest stored change first; the immutable ULID breaks equal-timestamp ties. */
export function byLatestChange(a: V2Task, b: V2Task) {
  return Date.parse(b.updatedAt) - Date.parse(a.updatedAt) || a.id.localeCompare(b.id);
}

export interface TaskGroups {
  needsYou: V2Task[];
  inFlight: V2Task[];
  closed: V2Task[];
}

/** Filters with the shared search first, then sorts each group by latest change. */
export function groupTasks(tasks: readonly V2Task[], filter: WorkspaceFilter): TaskGroups {
  const groups: TaskGroups = { needsYou: [], inFlight: [], closed: [] };
  for (const task of tasks) {
    if (!taskMatches(task, filter)) continue;
    groups[taskGroup(task.status)].push(task);
  }
  for (const group of Object.values(groups)) group.sort(byLatestChange);
  return groups;
}

/** The one attention predicate. A Grill Task needs you only on the user's turn, and it
    still belongs to In flight; `needs answer` is a pending T3 question, not Grilling. */
export function needsAttention(task: V2Task): boolean {
  if (NEEDS_YOU_STATUSES.has(task.status)) return true;
  return (
    task.status === "grilling" &&
    (task.grillTurn === "user_question" || task.grillTurn === "closing_approval")
  );
}

/** Unfiltered, deduplicated by id, newest change first. Used by the badge and Dock list. */
export function waitingTasks(tasks: readonly V2Task[]): V2Task[] {
  const seen = new Set<string>();
  const waiting: V2Task[] = [];
  for (const task of tasks) {
    if (seen.has(task.id)) continue;
    seen.add(task.id);
    if (needsAttention(task)) waiting.push(task);
  }
  return waiting.sort(byLatestChange);
}

export const needsYouCount = (tasks: readonly V2Task[]): number => waitingTasks(tasks).length;
