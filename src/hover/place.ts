import { availableMonitors, currentMonitor, getCurrentWindow, primaryMonitor, type Monitor } from "@tauri-apps/api/window";
import type { HoverEdge } from "../types";

/* Rust's `hover_place` takes a physical rect and computes nothing. The page owns the anchor: which
   monitor, which edge, and how far down the work area the tab sits. Expanding keeps the anchor and only
   clamps the bigger panel inside the work area, so collapsing puts the tab back where it was. */

/** A rectangle in physical pixels. */
export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Anchor {
  edge: HoverEdge;
  /** The top of the window, in physical pixels below the top of the work area. */
  offset: number;
}

/** Where a `width`×`height` window goes against `edge` of `area`, with its top pulled back inside the area. */
export function placeIn(area: Rect, anchor: Anchor, width: number, height: number): Rect {
  const w = Math.round(width);
  const h = Math.round(height);
  const x = anchor.edge === "left" ? area.x : area.x + area.width - w;
  const top = Math.min(area.y + anchor.offset, area.y + area.height - h);
  return { x: Math.round(x), y: Math.round(Math.max(area.y, top)), width: w, height: h };
}

/** The anchor for a window the user dropped at `at`: the nearer side edge, and its top as dropped. */
export function snap(area: Rect, at: Rect): Anchor {
  const centre = at.x + at.width / 2;
  const offset = Math.min(Math.max(at.y - area.y, 0), Math.max(area.height - at.height, 0));
  return { edge: centre < area.x + area.width / 2 ? "left" : "right", offset };
}

/* ── Tauri glue ── */

export interface Screen {
  /** `null` when the OS gives the monitor no name; it is then never saved. */
  name: string | null;
  area: Rect;
  scale: number;
}

function screenOf(m: Monitor): Screen {
  const { position, size } = m.workArea;
  return {
    name: m.name,
    area: { x: position.x, y: position.y, width: size.width, height: size.height },
    scale: m.scaleFactor,
  };
}

const KEY = "relay.hover.anchor";

interface Saved {
  last: string | null;
  monitors: Record<string, Anchor>;
}

function load(): Saved {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) return JSON.parse(raw) as Saved;
  } catch {
    /* No storage, or a value from an older shape: start from the default anchor. */
  }
  return { last: null, monitors: {} };
}

export function saveAnchor(screen: Screen, anchor: Anchor) {
  if (!screen.name) return;
  const saved = load();
  saved.last = screen.name;
  saved.monitors[screen.name] = anchor;
  try {
    localStorage.setItem(KEY, JSON.stringify(saved));
  } catch {
    /* The anchor still holds for this session. */
  }
}

/** The monitor the hover was last on, if still connected, and its saved anchor; else bottom right. */
export async function initialPlacement(): Promise<{ screen: Screen; anchor: Anchor } | null> {
  const saved = load();
  const monitors = await availableMonitors();
  const m =
    monitors.find((x) => saved.last !== null && x.name === saved.last) ??
    (await currentMonitor()) ??
    (await primaryMonitor());
  if (!m) return null;
  const screen = screenOf(m);
  const anchor = (screen.name && saved.monitors[screen.name]) || { edge: "right", offset: screen.area.height };
  return { screen, anchor };
}

/** After a drag: the monitor the window landed on and the edge it snaps to. Saved for that monitor. */
export async function placementAfterDrag(): Promise<{ screen: Screen; anchor: Anchor } | null> {
  const win = getCurrentWindow();
  const [pos, size, m] = await Promise.all([win.outerPosition(), win.outerSize(), currentMonitor()]);
  if (!m) return null;
  const screen = screenOf(m);
  const anchor = snap(screen.area, { x: pos.x, y: pos.y, width: size.width, height: size.height });
  saveAnchor(screen, anchor);
  return { screen, anchor };
}
