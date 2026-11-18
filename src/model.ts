import { useSyncExternalStore } from "react";
import type { Change, Claim, Due, Snapshot, Status, StatusCategory, Task } from "./types";

/** The only map from status category to colour. Rows, pills, the stepper and group headers read it. */
export function statusColor(category: StatusCategory) {
  return `var(--c-${category})`;
}

/* ── one shared clock: elapsed times tick every 30 s from a single timer ── */

const TICK_MS = 30_000;
let now = Date.now();
const listeners = new Set<() => void>();
let timer: number | undefined;

function subscribe(cb: () => void) {
  listeners.add(cb);
  if (timer === undefined) {
    timer = window.setInterval(() => {
      now = Date.now();
      listeners.forEach((l) => l());
    }, TICK_MS);
  }
  return () => {
    listeners.delete(cb);
    if (!listeners.size && timer !== undefined) {
      window.clearInterval(timer);
      timer = undefined;
    }
  };
}

export function useNow() {
  return useSyncExternalStore(subscribe, () => now);
}

/* ── formatting ── */

export function elapsed(fromIso: string, nowMs: number) {
  const m = Math.max(0, Math.round((nowMs - Date.parse(fromIso)) / 60_000));
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h}h ${m % 60}m` : `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

/** `1 project`, `2 projects`. */
export const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

export const clock = (iso: string) =>
  new Date(iso).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });

const startOfDay = (ms: number) => {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
};

export type DueTone = "today" | "late" | "later";

export function dueLabel(due: Due, nowMs: number): { text: string; tone: DueTone } {
  const at = Date.parse(due.at);
  const days = Math.round((startOfDay(at) - startOfDay(nowMs)) / 86_400_000);
  if (days < 0 || (days === 0 && due.hasTime && at < nowMs)) {
    return { text: weekdayDay(at), tone: "late" };
  }
  if (days === 0) return { text: due.hasTime ? `today ${clock(due.at)}` : "today", tone: "today" };
  return { text: weekdayDay(at), tone: "later" };
}

function weekdayDay(ms: number) {
  const d = new Date(ms);
  return `${d.toLocaleDateString("en-GB", { weekday: "short" }).toLowerCase()} ${d.getDate()}`;
}

export function headerDate(nowMs: number) {
  const d = new Date(nowMs);
  const day = d.toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short", year: "numeric" });
  return `${day} · ${clock(d.toISOString())}`;
}

export function until(iso: string, nowMs: number) {
  const m = Math.round((Date.parse(iso) - nowMs) / 60_000);
  if (m <= 0) return "now";
  const h = Math.floor(m / 60);
  return h ? `in ${h}h ${m % 60}m` : `in ${m}m`;
}

/* ── claims: fresh / stale / released, derived from last progress and the lease ── */

export const STALE_AFTER_MIN = 5;

export type ClaimState = "fresh" | "stale" | "released";

export function claimState(claim: Claim, nowMs: number): ClaimState {
  if (claim.releasedAt) return "released";
  return nowMs - Date.parse(claim.lastProgressAt) > STALE_AFTER_MIN * 60_000 ? "stale" : "fresh";
}

export const liveClaims = (tasks: Task[]) =>
  tasks.filter((t): t is Task & { claim: Claim } => !!t.claim && !t.claim.releasedAt);

/* ── one Today scope shared by the main header and the hover gauge ── */

export const isUnacked = (c: Change) => c.kind === "requirement" && !c.ackedAt;

export function statusOf(snapshot: Snapshot, task: Task): Status {
  return snapshot.statuses.find((s) => s.id === task.statusId) ?? snapshot.statuses[0];
}

export function todayScope(snapshot: Snapshot) {
  const tasks = snapshot.tasks.filter((t) => statusOf(snapshot, t).category !== "done");
  const count = (cat: StatusCategory) => tasks.filter((t) => statusOf(snapshot, t).category === cat).length;
  const unacked = tasks.flatMap((t) => t.changes.filter(isUnacked).map((change) => ({ task: t, change })));
  return {
    tasks,
    total: tasks.length,
    projects: new Set(tasks.map((t) => t.projectId)).size,
    inProgress: count("active"),
    blocked: count("blocked"),
    needsReview: snapshot.proposals.length,
    unacked,
    needsYou: snapshot.proposals.length + unacked.length,
  };
}

export function nextDue(snapshot: Snapshot, nowMs: number) {
  return todayScope(snapshot)
    .tasks.filter((t) => t.due?.hasTime && Date.parse(t.due.at) > nowMs)
    .sort((a, b) => Date.parse(a.due!.at) - Date.parse(b.due!.at))[0];
}
