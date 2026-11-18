import type { TaskLink } from "./types";

/* Pure readers for a PR link's synced state. The poller writes `metadataJson`; links it never
   reached (or older snapshots without the field) read as "none". */

export const CHECKS = ["none", "pending", "passing", "failing"] as const;
export const REVIEWS = ["none", "pending", "approved", "changes_requested"] as const;

export type Checks = (typeof CHECKS)[number];
export type Review = (typeof REVIEWS)[number];

export interface PrMeta {
  checks: Checks;
  review: Review;
}

function oneOf<T extends string>(value: unknown, allowed: readonly T[]): T {
  return allowed.includes(value as T) ? (value as T) : allowed[0];
}

export function prMeta(json: string | undefined): PrMeta {
  let raw: unknown;
  try {
    raw = JSON.parse(json ?? "{}");
  } catch {
    raw = null;
  }
  const o = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  return { checks: oneOf(o.checks, CHECKS), review: oneOf(o.review, REVIEWS) };
}

/** The PR's title; the snapshot falls back to the link kind when GitHub never answered. */
export function prTitle(link: TaskLink) {
  if (link.label && link.label !== link.kind) return link.label;
  return link.url.replace(/^https?:\/\/(www\.)?github\.com\//, "");
}
