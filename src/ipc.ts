import { invoke, isTauri } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { mockSnapshot } from "./data/mock";
import type { ProposalDecision, Snapshot } from "./types";

/* Typed wrappers over the Rust commands. Outside Tauri (pnpm dev in a browser), or while a command
   is not registered yet, each call resolves to its fallback so the UI keeps working on mock data.
   Any other error is real and propagates. */

export const inTauri = isTauri();

function isMissingCommand(error: unknown) {
  return /not found|unknown command/i.test(String(error));
}

async function call<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: () => T): Promise<T> {
  if (!inTauri) return fallback();
  try {
    return await invoke<T>(cmd, args);
  } catch (error) {
    if (isMissingCommand(error)) return fallback();
    throw error;
  }
}

/** The human text of a command error: Rust sends `{ kind, message }`. */
export function errorText(error: unknown) {
  if (error && typeof error === "object" && "message" in error) return String(error.message);
  return String(error);
}

export const getSnapshot = () => call<Snapshot>("relay_snapshot", undefined, () => structuredClone(mockSnapshot));

export const ackChange = (args: { taskId: string; changeId: string; rev: number }) =>
  call<void>("ack_change", args, () => undefined);

export const decideProposal = (args: { proposalId: string; decision: ProposalDecision; rev: number }) =>
  call<void>("decide_proposal", args, () => undefined);

/** Shows and focuses the main window as it is. */
export const showMainWindow = () =>
  call<void>("show_main_window", undefined, () => console.info("[relay] show_main_window"));

/** Shows the main window on a route from `src/route.ts`; Rust forwards it as `relay://open`. */
export const openMain = (route: string) =>
  call<void>("open_main", { route }, () => console.info(`[relay] open_main → ${route}`));

/** Asks the main window, which owns the updater, to run a manual check. */
export const requestUpdateCheck = () => call<void>("check_for_updates", undefined, () => undefined);

/* ── hover window ── */

/** The flags Rust keeps for the hover; `relay://state` carries the same shape. */
export interface HoverFlags {
  pinned: boolean;
  dnd: boolean;
  paused: boolean;
  ready: boolean;
  visible: boolean;
}

let browserFlags: HoverFlags = { pinned: false, dnd: false, paused: false, ready: true, visible: true };
const browserSet = (patch: Partial<HoverFlags>) => () => (browserFlags = { ...browserFlags, ...patch });

export const hoverReady = () => call<HoverFlags>("hover_ready", undefined, () => browserFlags);

export const hoverState = () => call<HoverFlags>("hover_state", undefined, () => browserFlags);

export const hoverSetPinned = (pinned: boolean) =>
  call<HoverFlags>("hover_set_pinned", { pinned }, browserSet({ pinned }));

export const setDnd = (dnd: boolean) => call<HoverFlags>("set_dnd", { dnd }, browserSet({ dnd }));

export const setPaused = (paused: boolean) => call<HoverFlags>("set_paused", { paused }, browserSet({ paused }));

/** Moves and sizes the hover in one step. Every value is in physical pixels. */
export const hoverPlace = (rect: { x: number; y: number; width: number; height: number }) =>
  call<void>("hover_place", rect, () => undefined);

/** Runs the OS move loop and resolves when the user lets go. */
export const hoverDrag = () => call<void>("hover_drag", undefined, () => undefined);

/** The cursor inside the hover in physical pixels, or null when it is outside or covered. */
export const hoverPointer = () => call<[number, number] | null>("hover_pointer", undefined, () => null);

/** Only for a field that needs the keyboard; pass false to hand focus back. */
export const hoverFocus = (focus: boolean) => call<void>("hover_focus", { focus }, () => undefined);

/* ── MCP ── */

/** The loopback MCP server as Rust reports it; `mcp://status` carries the same shape. Tokens are local
    bearer tokens for this machine, null until Rust has read or created the token files. */
export interface McpStatus {
  running: boolean;
  port: number;
  url: string;
  analystUrl: string;
  error: string | null;
  agentToken: string | null;
  analystToken: string | null;
}

const browserMcp: McpStatus = {
  running: false,
  port: 38547,
  url: "http://127.0.0.1:38547/mcp",
  analystUrl: "http://127.0.0.1:38547/mcp/analyst",
  error: null,
  agentToken: null,
  analystToken: null,
};

/** The last status; Rust starts the server in the background, so the first answer may say not running yet. */
export const mcpStatus = () => call<McpStatus>("mcp_status", undefined, () => ({ ...browserMcp }));

export const onMcpStatus = (handler: (status: McpStatus) => void) => onEvent("mcp://status", handler);

export const openExternal = async (url: string) => {
  if (!inTauri) {
    window.open(url, "_blank", "noopener");
    return;
  }
  try {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
  } catch (error) {
    console.warn("[relay] open failed", url, error);
  }
};

/* ── events ── */

/** What changed. Rust does not send it yet; the main window does after an ack or a decision. */
export interface ChangeEvent {
  taskId: string;
  changeId?: string;
  proposalId?: string;
}

interface Events {
  /** A route string for the main window. */
  "relay://open": string;
  "relay://change": ChangeEvent;
  "relay://state": HoverFlags;
  "mcp://status": McpStatus;
}

/** Subscribes to an app event. Returns an unsubscribe; a no-op in the browser. */
export function onEvent<K extends keyof Events>(name: K, handler: (payload: Events[K]) => void) {
  if (!inTauri) return () => {};
  let unlisten: (() => void) | undefined;
  let gone = false;
  listen<Events[K]>(name, (e) => handler(e.payload))
    .then((u) => {
      if (gone) u();
      else unlisten = u;
    })
    .catch((error) => console.warn(`[relay] listen ${name} failed`, error));
  return () => {
    gone = true;
    unlisten?.();
  };
}

/** Tells every window that a task or proposal changed, so the hover refreshes its counts. */
export async function emitChange(payload: ChangeEvent) {
  if (!inTauri) return;
  try {
    await emit("relay://change", payload);
  } catch (error) {
    console.warn("[relay] emit relay://change failed", error);
  }
}
