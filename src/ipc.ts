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

/** The loopback MCP server as Rust reports it; `mcp://status` carries the same shape. It holds no tokens:
    `mcpToken` fetches one when the user reveals or copies it. */
export interface McpStatus {
  running: boolean;
  port: number;
  url: string;
  analystUrl: string;
  error: string | null;
}

const browserMcp: McpStatus = {
  running: false,
  port: 38547,
  url: "http://127.0.0.1:38547/mcp",
  analystUrl: "http://127.0.0.1:38547/mcp/analyst",
  error: null,
};

/** The last status; Rust starts the server in the background, so the first answer may say not running yet. */
export const mcpStatus = () => call<McpStatus>("mcp_status", undefined, () => ({ ...browserMcp }));

/** Asks for one bearer token only when the user copies or reveals it. */
export const mcpToken = (which: "agent" | "analyst") =>
  call<string>("mcp_token", { which }, () => "");

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

/* -- Slack capture and JEV -- */

export interface SlackStatus {
  configured: boolean;
  connected: boolean;
  team: string | null;
  user: string | null;
  error: string | null;
  lastEventAt: string | null;
  capturePaused: boolean;
  peers: number;
  openEpisodes: number;
  transport: "claude" | "token";
  claude: ClaudeSlackStatus | null;
}

export interface ClaudeSlackStatus {
  found: boolean;
  path: string | null;
  running: boolean;
  lastRunAt: string | null;
  lastMessages: number;
  lastCostUsd: number;
  costTodayUsd: number;
  lastDurationMs: number;
  lastError: string | null;
}

export interface SlackSyncReport {
  conversations: number;
  messages: number;
  episodes: number;
  analyzed: number;
  error: string | null;
}

export interface SlackPeer {
  id: string;
  userId: string;
  name: string | null;
  isBot: boolean;
  excluded: boolean;
  createdAt: string;
}

export interface SlackMessage {
  id: string;
  episodeId: string;
  peerId: string;
  channelId: string;
  ts: string;
  threadTs: string | null;
  direction: "in" | "out";
  userId: string | null;
  text: string;
  permalink: string | null;
  editedAt: string | null;
  deleted: boolean;
  historical: boolean;
  createdAt: string;
}

export interface SlackEpisode {
  id: string;
  channelId: string;
  peerId: string;
  state: "open" | "waiting" | "closed" | "analyzed" | "needs_review";
  startedAt: string;
  lastMessageAt: string;
  closedAt: string | null;
  summary: string | null;
  jevJson: string | null;
  createdAt: string;
}

export interface EpisodeSummary {
  episode: SlackEpisode;
  peerName: string | null;
  peerUserId: string;
  messageCount: number;
  preview: string | null;
}

export interface EpisodeDetail {
  episode: SlackEpisode;
  peer: SlackPeer;
  messages: SlackMessage[];
}

export type AnalysisJobState = "pending" | "running" | "pending_agent" | "done" | "failed" | "superseded" | "shadow";

export interface AnalysisJob {
  id: string;
  conversationId: string;
  projectId: string | null;
  watermark: string;
  kind: "screen" | "classify";
  state: AnalysisJobState;
  attempts: number;
  route: "ignore" | "rescreen" | "review" | "change" | "new_task" | null;
  jevJson: string | null;
  candidates: string[];
  summary: string | null;
  error: string | null;
  supersededBy: string | null;
  agentSession: string | null;
  createdAt: string;
  updatedAt: string | null;
  startedAt: string | null;
  finishedAt: string | null;
}

export interface JevStatus {
  configured: boolean;
  model: string | null;
  error: string | null;
}

export interface GithubStatus {
  /** A token override is stored in Credential Manager. */
  configured: boolean;
  /** Where requests get credentials: the stored token, the gh CLI session, or nothing. */
  source: "token" | "gh" | "none";
  /** The GitHub login behind the gh session, when known. */
  login: string | null;
  error: string | null;
}

/** A project checkout's read-only git context. */
export interface GitState {
  branch: string | null;
  head: string | null;
  headSubject: string | null;
  dirty: boolean;
}

const browserSlackStatus: SlackStatus = {
  configured: false,
  connected: false,
  team: null,
  user: null,
  error: null,
  lastEventAt: null,
  capturePaused: false,
  peers: 0,
  openEpisodes: 0,
  transport: "token",
  claude: null,
};

export const slackSetTransport = (transport: "claude" | "token") =>
  call<SlackStatus>("slack_set_transport", { transport }, () => browserSlackStatus);

export const slackStatus = () => call<SlackStatus>("slack_status", undefined, () => browserSlackStatus);

export const slackSyncNow = () =>
  call<SlackSyncReport>("slack_sync_now", undefined, () => ({
    conversations: 0,
    messages: 0,
    episodes: 0,
    analyzed: 0,
    error: "Slack checking runs in the desktop app.",
  }));

export const slackConnect = (args: { userToken: string; appToken: string }) =>
  call<SlackStatus>("slack_connect", args, () => browserSlackStatus);

export const slackDisconnect = () =>
  call<SlackStatus>("slack_disconnect", undefined, () => browserSlackStatus);

export const slackPeers = () => call<SlackPeer[]>("slack_peers", undefined, () => []);

export const slackSetPeerExcluded = (args: { peerId: string; excluded: boolean }) =>
  call<SlackPeer | null>("slack_set_peer_excluded", args, () => null);

export const episodesList = (args: { stateFilter?: string; limit?: number } = {}) =>
  call<EpisodeSummary[]>("episodes_list", args, () => []);

export const episodeGet = (id: string) => call<EpisodeDetail | null>("episode_get", { id }, () => null);

export const episodeAnalyze = (id: string) =>
  call<AnalysisJob | null>("episode_analyze", { id }, () => null);

export const jobsList = (args: { stateFilter?: string; limit?: number } = {}) =>
  call<AnalysisJob[]>("jobs_list", args, () => []);

export const jobRetry = (id: string) => call<AnalysisJob | null>("job_retry", { id }, () => null);

export const jevStatus = () => call<JevStatus>("jev_status", undefined, () => ({ configured: false, model: null, error: null }));

export const jevSetKey = (key: string) =>
  call<JevStatus>("jev_set_key", { key }, () => ({ configured: false, model: null, error: null }));

export const jevClearKey = () => call<JevStatus>("jev_clear_key", undefined, () => ({ configured: false, model: null, error: null }));

export const githubStatus = () =>
  call<GithubStatus>("github_status", undefined, () => ({ configured: false, source: "none", login: null, error: null }));

export const githubSetToken = (token: string) =>
  call<GithubStatus>("github_set_token", { token }, () => ({ configured: false, source: "none", login: null, error: null }));

export const githubClearToken = () =>
  call<GithubStatus>("github_clear_token", undefined, () => ({ configured: false, source: "none", login: null, error: null }));

export const syncPrsNow = () => call<number>("sync_prs_now", undefined, () => 0);

export const projectGitState = (projectId: string) =>
  call<GitState | null>("project_git_state", { projectId }, () => null);

export const projectSetRepo = (projectId: string, repoPath: string | null) =>
  call<boolean>("project_set_repo", { projectId, repoPath }, () => false);

/** ClickUp and GitHub Issues import (one-shot, link-only). */
export interface ConnectorStatus {
  configured: boolean;
}

export interface ImportCount {
  imported: number;
  skipped: number;
}

const emptyImport: ImportCount = { imported: 0, skipped: 0 };

export const clickupStatus = () => call<ConnectorStatus>("clickup_status", undefined, () => ({ configured: false }));

export const clickupSetToken = (token: string) =>
  call<ConnectorStatus>("clickup_set_token", { token }, () => ({ configured: false }));

export const clickupClearToken = () =>
  call<ConnectorStatus>("clickup_clear_token", undefined, () => ({ configured: false }));

export const clickupImport = (args: { projectId: string; listId: string; includeClosed?: boolean }) =>
  call<ImportCount>("clickup_import", args, () => emptyImport);

export const githubImportIssues = (args: { projectId: string; owner: string; repo: string }) =>
  call<ImportCount>("github_import_issues", args, () => emptyImport);

interface Events {
  /** A route string for the main window. */
  "relay://open": string;
  "relay://change": ChangeEvent;
  "relay://state": HoverFlags;
  "mcp://status": McpStatus;
  "slack://status": SlackStatus;
  "github://status": GithubStatus;
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
