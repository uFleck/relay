import { invoke, isTauri } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { mockSnapshot } from "./data/mock";
import { sortNotes } from "./notes";
import type { ProviderKey } from "./route";
import { parseUrls } from "./references";
import type { LiveQueryResult, ReferenceParseResult, V2PickerRow } from "./references";
import type { ProposalDecision, RepoCandidate, Snapshot } from "./types";

/* Typed wrappers over the Rust commands. Outside Tauri (pnpm dev in a browser), or while a command
   is not registered yet, each call resolves to its fallback so the UI keeps working on mock data.
   Any other error is real and propagates. */

export const inTauri = isTauri();

function isMissingCommand(error: unknown) {
  return /not found|unknown command/i.test(String(error));
}

export async function call<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: () => T): Promise<T> {
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

/** The stable code behind a command error, used to detect a stale draft. */
export function errorKind(error: unknown): string | null {
  if (error && typeof error === "object" && "kind" in error) return String((error as { kind: unknown }).kind);
  return null;
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

/* ── notes ── */

/** A local note; Rust's `model::Note`. */
export interface Note {
  id: string;
  title: string;
  body: string;
  pinned: boolean;
  createdAt: string;
  updatedAt: string;
}

/* In the browser, notes live in memory so the view can be exercised on mock data. */
let browserNotes: Note[] = [];
const browserNote = (id: string) => {
  const note = browserNotes.find((n) => n.id === id);
  if (!note) throw { kind: "not_found", message: `note ${id}` };
  return note;
};

/** Pinned first, then most recently edited; a query filters by prefix over title and body. */
export const listNotes = (query: string) =>
  call<Note[]>("notes_list", { query: query.trim() || null }, () => {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    const hit = (n: Note) => {
      const tokens = `${n.title} ${n.body}`.toLowerCase().split(/\W+/);
      return words.every((w) => tokens.some((t) => t.startsWith(w)));
    };
    return sortNotes(browserNotes.filter(hit));
  });

export const createNote = (title = "", body = "") =>
  call<Note>("note_create", { title, body }, () => {
    const at = new Date().toISOString();
    const note = { id: crypto.randomUUID(), title, body, pinned: false, createdAt: at, updatedAt: at };
    browserNotes = [note, ...browserNotes];
    return note;
  });

export const updateNote = (args: { id: string; title: string; body: string }) =>
  call<Note>("note_update", args, () => ({ ...Object.assign(browserNote(args.id), { title: args.title, body: args.body, updatedAt: new Date().toISOString() }) }));

export const setNotePinned = (id: string, pinned: boolean) =>
  call<Note>("note_set_pinned", { id, pinned }, () => ({ ...Object.assign(browserNote(id), { pinned }) }));

export const deleteNote = (id: string) =>
  call<void>("note_delete", { id }, () => {
    browserNotes = browserNotes.filter((n) => n.id !== id);
  });

/** Creates a Relay task from the note; agents pick it up over MCP. Returns the new task's id. */
export const noteToTask = (id: string, projectId: string) =>
  call<{ id: string; title: string }>("note_to_task", { id, projectId }, () => ({ id: crypto.randomUUID(), title: browserNote(id).title }));

/** Brings T3 Code to the front (`t3code://`). It only opens the app; the prompt goes on the clipboard. */
export const openT3 = () => call<void>("open_t3", undefined, () => console.info("[relay] open_t3"));

/* ── v2 notebook ── */

export type V2AgentMode = "act" | "wait" | "grill";
export type V2TaskStatus =
  | "briefing" | "grilling" | "needs_project" | "awaiting_approval" | "dispatched"
  | "needs_answer" | "returned" | "failed" | "done" | "cancelled";
/** Whose move a Grilling Task waits on. Produced by #35, read by the attention selector. */
export type V2GrillTurn = "agent" | "user_question" | "closing_approval";

export interface V2Reference {
  id: string;
  kind: string;
  targetId: string;
  canonicalUrl: string;
  snapshotJson: string | null;
}

export interface V2NoteLine {
  id: string;
  text: string;
  position: number | null;
  references: V2Reference[];
}

export interface V2Task {
  id: string;
  sourceLineId: string;
  noteId: string;
  originalLine: string;
  agentMode: V2AgentMode;
  status: V2TaskStatus;
  projectId: string | null;
  brief: string | null;
  /** Only meaningful on a Grilling Task. Null is the same as "agent". */
  grillTurn: V2GrillTurn | null;
  pendingAction: string | null;
  failureStage: string | null;
  failureReason: string | null;
  threadUrl: string | null;
  references: V2Reference[];
  createdAt: string;
  updatedAt: string;
}

export interface V2Note {
  id: string;
  revision: number;
  source: string;
  body: string;
  createdAt: string;
  updatedAt: string;
  lines: V2NoteLine[];
  tasks: V2Task[];
}

export interface V2Project {
  id: string;
  name: string;
  slug: string;
  repoPath: string | null;
  ownerRepo: string | null;
  t3Target: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface V2Settings {
  importClickupEnabled: boolean;
  importGithubEnabled: boolean;
  importScope: string | null;
  updatedAt: string;
}

export interface V2NotebookSnapshot {
  notes: V2Note[];
  projects: V2Project[];
  settings: V2Settings;
}

/** The one read behind both v2 views. `tasks` is flat and unfiltered. */
export interface V2WorkspaceSnapshot extends V2NotebookSnapshot {
  tasks: V2Task[];
}

/** Ids from the shared filter path; the caller renders from its workspace snapshot. */
export interface WorkspaceSearchResult {
  query: string;
  noteIds: string[];
  taskIds: string[];
}

export type { ProviderKey };

export interface V2LineInput {
  lineId: string | null;
  text: string;
}

export type V2ComposerSaveRequest =
  | { kind: "create"; submissionKey: string; text: string }
  | { kind: "edit"; noteId: string; expectedRevision: number; lines: V2LineInput[] };

const emptyV2Snapshot = (): V2NotebookSnapshot => ({
  notes: [],
  projects: [],
  settings: {
    importClickupEnabled: false,
    importGithubEnabled: false,
    importScope: null,
    updatedAt: new Date(0).toISOString(),
  },
});

export const v2NotebookSnapshot = () =>
  call<V2NotebookSnapshot>("v2_notebook_snapshot", undefined, emptyV2Snapshot);

export const v2WorkspaceSnapshot = () =>
  call<V2WorkspaceSnapshot>("v2_workspace_snapshot", undefined, () => ({
    ...emptyV2Snapshot(),
    tasks: [],
  }));

export const v2WorkspaceSearch = (query: string) =>
  call<WorkspaceSearchResult>("v2_workspace_search", { query }, () => ({
    query,
    noteIds: [],
    taskIds: [],
  }));

export type {
  Chip,
  EditorState,
  ReferenceAvailability,
  ReferenceParseResult,
  ReferenceSnapshot,
  LiveQueryResult,
  ProviderProgress,
  ProviderProgressState,
  UrlMatch,
  V2PickerGroup,
  V2PickerRow,
} from "./references";

/* These two are registered through Tauri and never fall back to a mock while running in
   the app: a missing command must fail loudly instead of looking like an empty picker. */
export async function v2ReferencePicker(query: string): Promise<V2PickerRow[]> {
  if (!inTauri) return [];
  return await invoke<V2PickerRow[]>("v2_reference_picker", { query });
}

export async function v2ReferenceParse(text: string): Promise<ReferenceParseResult> {
  if (!inTauri) return parseUrls(text);
  return await invoke<ReferenceParseResult>("v2_reference_parse", { text });
}

/** A bounded live query. Local results stay usable while this resolves; the query on the
    result lets the picker drop a response whose query has since changed. */
export async function v2ReferenceSearch(query: string): Promise<LiveQueryResult> {
  if (!inTauri) return { query, rows: [], progress: [] };
  return await invoke<LiveQueryResult>("v2_reference_search", { query });
}

/** Deduplicated exact read for a pasted or linked key. A miss leaves any cached copy in place. */
export async function v2ReferenceExact(key: string, canonicalUrl: string | null): Promise<V2PickerRow | null> {
  if (!inTauri) return null;
  return await invoke<V2PickerRow | null>("v2_reference_exact", { key, canonicalUrl });
}

/** Persists an accepted live result as a read-only snapshot. No Note, Task or ledger row. */
export async function v2ReferencePick(row: V2PickerRow): Promise<void> {
  if (!inTauri) return;
  await invoke<void>("v2_reference_pick", { row });
}

/** A snapshot changed (picked or recovered) in either window. */
export const onReferenceChange = (handler: (key: string) => void) =>
  onEvent("relay://reference-change", handler);

/* Writes never fall back to a mock: a missing command or a stale revision must surface as
   an error, not as a note that looks saved. In a plain browser there is no store at all. */
export async function v2ComposerSave(request: V2ComposerSaveRequest): Promise<V2Note> {
  if (!inTauri) throw new Error("Saving a note requires the Relay desktop app.");
  return await invoke<V2Note>("v2_composer_save", { request });
}

export async function v2TaskAssignProject(taskId: string, projectId: string): Promise<V2Task> {
  if (!inTauri) throw new Error("Assigning a project requires the Relay desktop app.");
  return await invoke<V2Task>("v2_task_assign_project", { taskId, projectId });
}

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

export interface ReanalyzeReport {
  selected: number;
  analyzed: number;
  failed: number;
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

export const slackReanalyze = (conversationId: string) =>
  call<AnalysisJob | null>("slack_reanalyze", { conversationId }, () => null);

export const slackReanalyzeRecent = () =>
  call<ReanalyzeReport>("slack_reanalyze_recent", undefined, () => ({
    selected: 0,
    analyzed: 0,
    failed: 0,
    error: "Re-analysis runs in the desktop app.",
  }));

export const jobsList = (args: { stateFilter?: string; limit?: number } = {}) =>
  call<AnalysisJob[]>("jobs_list", args, () => []);

export const jobRetry = (id: string) => call<AnalysisJob | null>("job_retry", { id }, () => null);
/** Settles a job waiting in Review: `send` starts the reviewer on it, otherwise it is dismissed. */
export const jobResolveReview = (id: string, send: boolean) =>
  call<AnalysisJob | null>("job_resolve_review", { id, send }, () => null);

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

export interface ClickupNamed {
  id: string;
  name: string;
}

export interface ClickupScope {
  workspaceId: string;
  /** Null tracks the whole workspace; guests have no spaces to choose. */
  spaceId: string | null;
  projectId: string;
}

export interface ClickupSyncResult {
  created: number;
  updated: number;
  archived: number;
  truncated: boolean;
}

export interface ClickupScopeView {
  scope: ClickupScope | null;
  last: ClickupSyncResult | null;
  error: string | null;
}

export const clickupWorkspaces = () => call<ClickupNamed[]>("clickup_workspaces", undefined, () => []);

export const clickupSpaces = (workspaceId: string) =>
  call<ClickupNamed[]>("clickup_spaces", { workspaceId }, () => []);

export const clickupScope = () =>
  call<ClickupScopeView>("clickup_scope", undefined, () => ({ scope: null, last: null, error: null }));

export const clickupSetScope = (scope: ClickupScope) => call<void>("clickup_set_scope", { scope }, () => undefined);

export const clickupSyncNow = () =>
  call<ClickupSyncResult>("clickup_sync_now", undefined, () => ({ created: 0, updated: 0, archived: 0, truncated: false }));

export const githubImportIssues = (args: { projectId: string; owner: string; repo: string }) =>
  call<ImportCount>("github_import_issues", args, () => emptyImport);

export const reposScan = (folders: string[]) => call<RepoCandidate[]>("repos_scan", { folders }, () => []);

export const reposImport = (folders: string[]) => call<ImportCount>("repos_import", { folders }, () => emptyImport);

interface Events {
  /** A route string for the main window. */
  "relay://open": string;
  "relay://change": ChangeEvent;
  /** A committed v2 write; the payload is the saved Note id. */
  "relay://v2-change": string;
  "relay://state": HoverFlags;
  "mcp://status": McpStatus;
  "slack://status": SlackStatus;
  "github://status": GithubStatus;
  /** Secret-free provider status map from the v2 Settings boundary. */
  "relay://settings-status": import("./settings").ProviderStatusMap;
  /** A v2 setting (Import scope, project mapping, OpenCode model) was saved. */
  "relay://settings-change": null;
  /** A picked or recovered Reference snapshot changed; the payload is its canonical key. */
  "relay://reference-change": string;
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

/** A committed v2 write in either window. Both v2 views refresh from this shared signal. */
export const onV2Change = (handler: (noteId: string) => void) => onEvent("relay://v2-change", handler);

/** Tells every window that a task or proposal changed, so the hover refreshes its counts. */
export async function emitChange(payload: ChangeEvent) {
  if (!inTauri) return;
  try {
    await emit("relay://change", payload);
  } catch (error) {
    console.warn("[relay] emit relay://change failed", error);
  }
}
