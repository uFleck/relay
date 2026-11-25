import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ProviderKey } from "./route";
import { parseUrls } from "./references";
import { visualFixturePickerRows, visualFixtureWorkspace } from "./visual-fixture";
import type {
  LiveQueryResult,
  ReferenceParseResult,
  SlackSearchHit,
  V2PickerRow,
} from "./references";

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

/** Shows and focuses the main window as it is. */
export const showMainWindow = () =>
  call<void>("show_main_window", undefined, () => console.info("[relay] show_main_window"));

/** Shows the main window on a route from `src/route.ts`; Rust forwards it as `relay://open`. */
export const openMain = (route: string) =>
  call<void>("open_main", { route }, () => console.info(`[relay] open_main → ${route}`));

/** Asks the main window, which owns the updater, to run a manual check. */
export const requestUpdateCheck = () => call<void>("check_for_updates", undefined, () => undefined);

/** App version from Cargo, shared by the rail, Settings and the Dock. */
export const appVersion = () => call<string>("app_version", undefined, () => "0.0.0");

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

export const hoverSetPinned = (pinned: boolean) =>
  call<HoverFlags>("hover_set_pinned", { pinned }, browserSet({ pinned }));

export const setDnd = (dnd: boolean) => call<HoverFlags>("set_dnd", { dnd }, browserSet({ dnd }));

export const setPaused = (paused: boolean) => call<HoverFlags>("set_paused", { paused }, browserSet({ paused }));

/* ── Dock session (native activation, measured height, dismissal) ── */

/** The Dock-only focus/session signal. The page focuses the input only on a verified activation. */
export interface DockFocusEvent {
  sessionId: number;
  visible: boolean;
  focused: boolean;
  hint: "click-to-focus" | null;
}

/** Registers the page's listeners and reports the measured compact CSS height. */
export const dockReady = (height: number) => call<boolean>("dock_ready", { height }, () => true);

/** Applies a measured CSS height to a session; the reply arrives after the native resize. */
export const dockResize = (height: number, sessionId: number) =>
  call<boolean>("dock_resize", { height, sessionId }, () => false);

/** Explicit dismissal from an unconsumed Escape or a successful local save. */
export const dockDismiss = (sessionId: number, reason: "escape" | "saved") =>
  call<boolean>("dock_dismiss", { sessionId, reason }, () => false);

export const onDockFocus = (handler: (event: DockFocusEvent) => void) =>
  onEvent("relay://dock-focus", handler);

/** A native scale-factor change moved or rescaled the work area; remeasure against the new DPI. */
export interface DockDpiEvent {
  sessionId: number;
}

export const onDockDpi = (handler: (event: DockDpiEvent) => void) =>
  onEvent("relay://dock-dpi", handler);

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

/* ── Briefing (#34) ── */

/** How one Reference contributed to this attempt, as reported by the core plan. */
export type V2BriefRefInput = "content" | "cached" | "linkOnly" | "missing";

export interface V2BriefRefView {
  identity: string;
  canonicalUrl: string;
  title: string | null;
  input: V2BriefRefInput;
  summary: string | null;
}

export interface V2BriefContent {
  goal: string;
  decisions: string[];
  constraints: string[];
  openRisks: string[];
  conversationSummaries: { identity: string; summary: string }[];
}

/** One read drives the inline Brief panel in both the main window and the Dock. */
export interface V2BriefView {
  taskId: string;
  mode: V2AgentMode;
  taskStatus: V2TaskStatus;
  briefRevision: number | null;
  rendered: string | null;
  content: V2BriefContent;
  conversationSummaries: { identity: string; summary: string }[];
  references: V2BriefRefView[];
  missingReferences: string[];
  canGo: boolean;
  canRewrite: boolean;
  canRetry: boolean;
  projectId: string | null;
  failureStage: string | null;
  failureReason: string | null;
  rewriteInstruction: string | null;
  pendingAction: string | null;
  dispatchReady: boolean;
  updatedAt: string | null;
}

export const v2BriefingView = (taskId: string) =>
  call<V2BriefView | null>("v2_briefing_view", { taskId }, () => null);

/** Starts one generation for a Task line. Concurrent callers share the running attempt. */
export const v2BriefingGenerate = (taskId: string) =>
  call<V2BriefView | null>("v2_briefing_generate", { taskId }, () => null);

/** Go: approves the exact revision, including edits submitted with it. */
export const v2BriefingApprove = (taskId: string, revision: number, edited: string | null) =>
  call<V2BriefView | null>("v2_briefing_approve", { taskId, revision, edited }, () => null);

/** Requests an OpenCode rewrite. The last saved Brief stays until the new one validates. */
export const v2BriefingRewrite = (taskId: string, revision: number, instruction: string) =>
  call<V2BriefView | null>("v2_briefing_rewrite", { taskId, revision, instruction }, () => null);

/** Explicit choice to proceed with a link when a Reference has no content. */
export const v2BriefingContinueLinkOnly = (taskId: string, revision: number, identity: string) =>
  call<V2BriefView | null>(
    "v2_briefing_continue_link_only",
    { taskId, revision, identity },
    () => null,
  );

/** Re-runs the failed Briefing stage. */
export const v2BriefingRetry = (taskId: string) =>
  call<V2BriefView | null>("v2_briefing_retry", { taskId }, () => null);

/* ── Grilling (#35) ── */

export type V2GrillStatus = "pending" | "question" | "brief" | "failed" | "approved" | "cancelled";
export type V2GrillRole = "question" | "answer";

export interface V2GrillEntry {
  role: V2GrillRole;
  text: string;
  createdAt: string;
}

/** One saved exchange. A Task-line edit starts a new revision and keeps the earlier one. */
export interface V2GrillExchange {
  revision: number;
  entries: V2GrillEntry[];
}

/** The whole Grilling read for one Task. It carries no token, bearer or Reference content. */
export interface V2GrillView {
  taskId: string;
  mode: V2AgentMode;
  taskStatus: V2TaskStatus;
  revision: number | null;
  status: V2GrillStatus | null;
  question: string | null;
  entries: V2GrillEntry[];
  exchanges: V2GrillExchange[];
  missingReferences: string[];
  canAnswer: boolean;
  canRetry: boolean;
  canCancel: boolean;
  canGo: boolean;
  dispatchReady: boolean;
  projectId: string | null;
  rendered: string | null;
  pendingAction: string | null;
  failureStage: string | null;
  failureReason: string | null;
  updatedAt: string | null;
}

export const v2GrillingView = (taskId: string) =>
  call<V2GrillView | null>("v2_grilling_view", { taskId }, () => null);

/** Starts one Grilling exchange for a fresh ?? Task. Concurrent callers share the attempt. */
export const v2GrillingStart = (taskId: string) =>
  call<V2GrillView | null>("v2_grilling_start", { taskId }, () => null);

/** Persists one answer before the next model turn is claimed. */
export const v2GrillingAnswer = (taskId: string, revision: number, answer: string) =>
  call<V2GrillView | null>("v2_grilling_answer", { taskId, revision, answer }, () => null);

/** Re-runs the failed turn with the saved transcript replayed. */
export const v2GrillingRetry = (taskId: string) =>
  call<V2GrillView | null>("v2_grilling_retry", { taskId }, () => null);

/** Cancels the exchange without deleting its saved transcript. */
export const v2GrillingCancel = (taskId: string) =>
  call<V2GrillView | null>("v2_grilling_cancel", { taskId }, () => null);

/** The single closing approval, bound to the displayed Brief revision. */
export const v2GrillingGo = (taskId: string, revision: number, edited: string | null) =>
  call<V2GrillView | null>("v2_grilling_go", { taskId, revision, edited }, () => null);

export const v2GrillingContinueLinkOnly = (taskId: string, revision: number, identity: string) =>
  call<V2GrillView | null>(
    "v2_grilling_continue_link_only",
    { taskId, revision, identity },
    () => null,
  );

/* ── T3 dispatch (#36) ── */

export type V2DispatchState = "launching" | "launch_failed" | "reconcile_required" | "bound";

/** The dispatch read for a Task sheet. Buttons come from the stored state, never from a CLI probe. */
export interface V2DispatchView {
  taskId: string;
  state: V2DispatchState;
  attemptId: string;
  threadId: string | null;
  threadUrl: string | null;
  failureReason: string | null;
  canRetry: boolean;
  canCheckAgain: boolean;
  canLinkExisting: boolean;
}

export const v2DispatchView = (taskId: string) =>
  call<V2DispatchView | null>("v2_dispatch_view", { taskId }, () => null);

/** Manual Retry after a known launch failure. An uncertain attempt is still fenced by core. */
export const v2DispatchRetry = (taskId: string, baseRef: string | null = null) =>
  call<void>("v2_dispatch_retry", { taskId, baseRef }, () => undefined);

/** Read-only reconciliation: never starts a second thread. */
export const v2DispatchCheck = (taskId: string) =>
  call<void>("v2_dispatch_check", { taskId }, () => undefined);

/** Links a thread the user compared with the frozen Brief in T3. */
export const v2DispatchLink = (taskId: string, threadId: string, userConfirmed: boolean) =>
  call<void>("v2_dispatch_link", { taskId, threadId, userConfirmed }, () => undefined);

/** Stops the UI wait; a response already in flight still records its thread identity. */
export const v2DispatchCancel = (taskId: string) =>
  call<void>("v2_dispatch_cancel", { taskId }, () => undefined);

/* ── T3 progress and continuation (#37) ── */

export interface V2ObservedPullRequest {
  url: string;
  source: string | null;
  state: string | null;
  watching: boolean;
}

export type V2SendState = "reserved" | "acknowledged" | "needs_confirmation" | "failed" | "stale";

export interface V2SendOperation {
  operationId: string;
  attemptId: string;
  taskId: string;
  threadId: string;
  kind: "followup" | "retry" | "resend";
  state: V2SendState;
  /** The user's exact multiline message, frozen before the CLI ran. */
  message: string;
  baselineRunId: string | null;
  baselineRunCount: number;
  runId: string | null;
  deliveryState: string | null;
  failureReason: string | null;
  revision: number;
}

export interface V2T3Progress {
  taskId: string;
  attemptId: string;
  threadId: string;
  threadUrl: string | null;
  runId: string | null;
  runCount: number;
  /** Last observed T3 run status; kept even when the Task status is unchanged. */
  status: string;
  taskStatus: V2TaskStatus;
  pendingRequestCount: number;
  branch: string | null;
  worktreePath: string | null;
  reply: string | null;
  error: string | null;
  staleReason: string | null;
  revision: number;
  pullRequests: V2ObservedPullRequest[];
  activeSend: V2SendOperation | null;
  canFollowUp: boolean;
  canRetryRun: boolean;
  canClose: boolean;
  canOpenT3: boolean;
  canCheckAgain: boolean;
  canResend: boolean;
}

export interface V2T3SendResult {
  outcome: "sent" | "draft_unsent" | "draft_kept" | "needs_confirmation" | "fenced" | "busy";
  message: string;
  runId: string | null;
}

export interface V2T3CheckResult {
  confirmed: boolean;
}

export const v2T3Progress = (taskId: string) =>
  call<V2T3Progress | null>("v2_t3_progress", { taskId }, () => null);

/** One metadata refresh when an open Returned or Failed sheet mounts. */
export const v2T3Refresh = (taskId: string) =>
  call<void>("v2_t3_refresh", { taskId }, () => undefined);

/** Follow-up or Retry on the same thread. The server pins the exact Retry sentence. */
export const v2T3Send = (taskId: string, message: string, kind: "followup" | "retry") =>
  call<V2T3SendResult>("v2_t3_send", { taskId, message, kind }, () => ({
    outcome: "busy" as const,
    message: "T3 progress is unavailable in this preview.",
    runId: null,
  }));

export const v2T3CheckAgain = (taskId: string) =>
  call<V2T3CheckResult>("v2_t3_check_again", { taskId }, () => ({ confirmed: false }));

export const v2T3Resend = (taskId: string, userConfirmed: boolean) =>
  call<V2T3SendResult>("v2_t3_resend", { taskId, userConfirmed }, () => ({
    outcome: "busy" as const,
    message: "T3 progress is unavailable in this preview.",
    runId: null,
  }));

/** Explicit local closure only. A PR merge or T3 settlement never calls this. */
export const v2T3Close = (taskId: string, status: "done" | "cancelled") =>
  call<void>("v2_t3_close", { taskId, status }, () => undefined);

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

export const isVisualFixture = () =>
  typeof location !== "undefined" &&
  new URLSearchParams(location.search).get("visual-fixture") === "J";

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
  call<V2WorkspaceSnapshot>("v2_workspace_snapshot", undefined, () =>
    isVisualFixture()
      ? visualFixtureWorkspace()
      : {
          ...emptyV2Snapshot(),
          tasks: [],
        },
  );

export const v2WorkspaceSearch = (query: string) =>
  call<WorkspaceSearchResult>("v2_workspace_search", { query }, () => ({
    query,
    noteIds: [],
    taskIds: [],
  }));

/** The result of a create-only Note Import run, one line per enabled source. */
export interface ImportRunResult {
  created: number;
  skipped: number;
  truncated: boolean;
  clickupError: string | null;
  githubError: string | null;
}

export const v2ImportNow = () =>
  call<ImportRunResult>("v2_import_now", undefined, () => ({
    created: 0,
    skipped: 0,
    truncated: false,
    clickupError: null,
    githubError: null,
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
  if (!inTauri) return isVisualFixture() ? visualFixturePickerRows(query) : [];
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

/* ── Slack directory, explicit search and frozen windows (#33) ── */

export interface SlackDirectoryReport {
  conversations: number;
  invocations: number;
  costUsd: number;
  scannedAt: string | null;
  error: string | null;
  coalesced: boolean;
}

export interface SlackSearchReport {
  query: string;
  hits: SlackSearchHit[];
  error: string | null;
}

export interface SlackDirectoryProgress {
  state: string;
  conversations: number;
  error: string | null;
}

/** Manual Refresh conversations. The core single-flight guard coalesces a second press. */
export const v2SlackRefresh = () =>
  call<SlackDirectoryReport>(
    "v2_slack_refresh",
    undefined,
    () => ({
      conversations: 0,
      invocations: 0,
      costUsd: 0,
      scannedAt: null,
      error: "Conversations refresh runs in the desktop app.",
      coalesced: false,
    }),
  );

/** Enter-driven explicit search. Typing never calls this. */
export async function v2SlackSearch(query: string): Promise<SlackSearchReport> {
  if (!inTauri) return { query, hits: [], error: null };
  return await invoke<SlackSearchReport>("v2_slack_search", { query });
}

/** Creates a distinct frozen-window Reference and returns the immediate "fetching…" chip row. */
export async function v2SlackWindowStart(
  channelId: string,
  host: string | null,
  name: string | null,
): Promise<V2PickerRow | null> {
  if (!inTauri) return null;
  return await invoke<V2PickerRow | null>("v2_slack_window_start", { channelId, host, name });
}

/** A snapshot changed (picked or recovered) in either window. */
export const onReferenceChange = (handler: (key: string) => void) =>
  onEvent("relay://reference-change", handler);

/** Directory scan progress. Kept separate from provider search feedback. */
export const onSlackDirectory = (handler: (progress: SlackDirectoryProgress) => void) =>
  onEvent("relay://slack-directory", handler);

/* Writes never fall back to a mock: a missing command or a stale revision must surface as
   an error, not as a note that looks saved. In a plain browser there is no store at all. */
export async function v2NoteDelete(noteId: string): Promise<void> {
  return invoke("v2_note_delete", { noteId });
}

export async function v2TaskDelete(taskId: string): Promise<void> {
  return invoke("v2_task_delete", { taskId });
}

export async function v2ComposerSave(request: V2ComposerSaveRequest): Promise<V2Note> {
  if (!inTauri) throw new Error("Saving a note requires the Relay desktop app.");
  return await invoke<V2Note>("v2_composer_save", { request });
}

export async function v2TaskAssignProject(taskId: string, projectId: string): Promise<V2Task> {
  if (!inTauri) throw new Error("Assigning a project requires the Relay desktop app.");
  return await invoke<V2Task>("v2_task_assign_project", { taskId, projectId });
}

/* ── events ── */

interface Events {
  /** A route string for the main window. */
  "relay://open": string;
  /** A committed v2 write; the payload is the saved Note id. */
  "relay://v2-change": string;
  "relay://state": HoverFlags;
  "relay://dock-focus": DockFocusEvent;
  "relay://dock-dpi": DockDpiEvent;
  /** Secret-free provider status map from the v2 Settings boundary. */
  "relay://settings-status": import("./settings").ProviderStatusMap;
  /** A v2 setting (Import scope, project mapping, OpenCode model) was saved. */
  "relay://settings-change": null;
  /** The configurable keyboard shortcuts changed. */
  "relay://keybinds-change": import("./keybinds").Keybinds;
  /** A picked or recovered Reference snapshot changed; the payload is its canonical key. */
  "relay://reference-change": string;
  /** Directory scan progress, shared by the Dock and Notebook pickers. */
  "relay://slack-directory": SlackDirectoryProgress;
  /** A Brief was generated, edited, approved, retried or recovered. Payload is the Task id; empty means restart recovery. */
  "relay://briefing": string;
  /** A Grilling question, answer, closing approval or cancellation changed. Payload is the Task id. */
  "relay://grilling": string;
  /** A T3 dispatch claim, failure or reconciliation changed. Payload is the Task id. */
  "relay://dispatch": string;
  /** An observed run, pending question, continuation or local closure changed. */
  "relay://t3-progress": string;
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
