import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { EMPTY_DRAFT, type DraftSummary } from "../directives";
import { Composer } from "../components/Composer";
import { BriefPanel } from "../components/BriefPanel";
import { GrillPanel } from "../components/GrillPanel";
import { Icon } from "../components/Icon";
import {
  appVersion,
  dockDismiss,
  dockReady,
  dockResize,
  inTauri,
  onDockDpi,
  onDockFocus,
  onEvent,
  onV2Change,
  openMain,
  requestUpdateCheck,
  setDnd,
  showMainWindow,
  v2WorkspaceSnapshot,
  type HoverFlags,
  type V2AgentMode,
  type V2WorkspaceSnapshot,
} from "../ipc";
import { taskRoute } from "../route";
import type { HoverEdge } from "../types";
import {
  DockContext,
  dockCssHeight,
  dockLayoutState,
  SUMMON_CHORD,
  type DockBridge,
  type DockLayoutState,
} from "./dock";
import { prefersReducedMotion } from "../ui/motion";
import { followUpdates, requestInstall, useUpdater, type UpdateState } from "../updater";
import { needsYouCount, waitingTasks } from "../workspace";

/* The Dock reads only the v2 workspace. The retired v1 snapshot, claim board and Slack
   source row are gone; the native session and focus machinery below is unchanged. */
const REFRESH_MS = 20_000;

export function HoverApp() {
  const [v2, setV2] = useState<V2WorkspaceSnapshot | null>(null);
  const [version, setVersion] = useState("");
  const [failed, setFailed] = useState(false);
  const [flags, setFlags] = useState<HoverFlags>({ pinned: false, dnd: false, paused: false, ready: false, visible: false });
  const [pulsing, setPulsing] = useState(false);
  const [composing, setComposing] = useState(true);
  const [showWaiting, setShowWaiting] = useState(false);
  /** A Wait or Grill Task with its inline panel open in the Dock; empty when none is open. */
  const [dockBrief, setDockBrief] = useState<string | null>(null);
  const [dockLayout, setDockLayout] = useState<DockLayoutState>("compact");
  const [draft, setDraft] = useState<DraftSummary>(EMPTY_DRAFT);
  const [dockVisible, setDockVisible] = useState(!inTauri);
  const [focusHint, setFocusHint] = useState(false);
  const [growing, setGrowing] = useState(false);
  const composerActive = useRef(false);
  const update = useUpdater();
  const shell = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const escConsumed = useRef(false);
  const waitingIds = useRef<Set<string> | null>(null);
  // Native session plumbing. Every queued measurement carries the session id it started with.
  const sessionRef = useRef<number | null>(null);
  const layoutRef = useRef(dockLayout);
  layoutRef.current = dockLayout;
  const pickerOpenRef = useRef(false);
  const sentHeight = useRef<number | null>(null);
  const readySent = useRef(false);
  // The pointer poll and the timers read these without restarting.
  const live = useRef({ dnd: flags.dnd, composer: false, waiting: false, brief: false });
  live.current = {
    dnd: flags.dnd,
    composer: composerActive.current,
    waiting: showWaiting,
    brief: dockBrief !== null,
  };

  const pulse = useCallback(() => {
    if (live.current.dnd) return;
    // Remove then re-add the class so a change mid-pulse restarts the 3 beats.
    setPulsing(false);
    requestAnimationFrame(() => setPulsing(true));
  }, []);

  const refreshV2 = useCallback(async () => {
    try {
      const workspace = await v2WorkspaceSnapshot();
      const ids = new Set(waitingTasks(workspace.tasks).map((task) => task.id));
      if (waitingIds.current && [...ids].some((id) => !waitingIds.current!.has(id))) pulse();
      waitingIds.current = ids;
      setV2(workspace);
      setFailed(false);
    } catch (error) {
      console.warn("[relay] hover v2 workspace", error);
      setFailed(true);
    }
  }, [pulse]);

  /** Explicit dismissal keeps the draft: the Composer stays mounted and only the native window hides. */
  const dismissDock = useCallback(async (reason: "escape" | "saved") => {
    const id = sessionRef.current;
    if (id === null) return;
    sessionRef.current = null;
    setDockVisible(false);
    setFocusHint(false);
    await dockDismiss(id, reason).catch(() => undefined);
  }, []);

  /** The picker is absolutely positioned above the input, so its settled box is added here. */
  const measureHeight = useCallback(() => {
    const content = contentRef.current;
    if (!content) return 0;
    const picker = content.querySelector<HTMLElement>(".ref-picker");
    const pickerHeight = picker && layoutRef.current !== "compact" ? picker.scrollHeight : 0;
    const measured = Math.max(content.getBoundingClientRect().height, content.scrollHeight);
    return dockCssHeight(measured, pickerHeight, layoutRef.current);
  }, []);

  /** A close transition must finish before the window shrinks. Reduced motion skips the wait. */
  const waitForDockTransition = useCallback(() => {
    if (prefersReducedMotion()) return Promise.resolve();
    return new Promise<void>((resolve) => {
      let timer = 0;
      const done = () => {
        window.removeEventListener("relay://dock-transition-end", done);
        window.clearTimeout(timer);
        resolve();
      };
      window.addEventListener("relay://dock-transition-end", done);
      timer = window.setTimeout(done, 450);
    });
  }, []);

  const applyHeight = useCallback(
    async (force = false) => {
      if (!inTauri) return;
      const height = measureHeight();
      if (!height) return;
      if (!readySent.current) {
        readySent.current = true;
        sentHeight.current = height;
        await dockReady(height).catch(() => undefined);
        return;
      }
      const previous = sentHeight.current;
      if (!force && previous === height) return;
      const id = sessionRef.current;
      if (id === null) {
        // No session to resize. Re-measure on the next summon instead.
        sentHeight.current = null;
        return;
      }
      const growing = previous === null || height > previous;
      if (growing) setGrowing(true);
      else await waitForDockTransition();
      if (sessionRef.current !== id) {
        setGrowing(false);
        return;
      }
      const ok = await dockResize(height, id).catch(() => false);
      if (ok) sentHeight.current = height;
      setGrowing(false);
    },
    [measureHeight, waitForDockTransition],
  );

  /* Everything #39 needs to size and focus the native box: the measured content root, the
     three layout states, the picker's settled event, the live input and the save/escape signals. */
  const dock = useMemo<DockBridge>(
    () => ({
      viewportRef,
      contentRef,
      inputRef,
      layout: dockLayout,
      reportPicker: (open, lensVisible) => {
        pickerOpenRef.current = open;
        const next = dockLayoutState(open, lensVisible);
        setDockLayout((current) => (current === next ? current : next));
      },
      escConsumed,
      reportEsc: () => {
        escConsumed.current = true;
      },
      takeEsc: () => {
        const taken = escConsumed.current;
        escConsumed.current = false;
        return taken;
      },
      onPickerTransitionEnd: () => {
        window.dispatchEvent(new CustomEvent("relay://dock-transition-end", { detail: { layout: dockLayout } }));
      },
      onSaved: () => {
        void dismissDock("saved");
        void refreshV2();
      },
      reportDraft: setDraft,
    }),
    [dockLayout, refreshV2, dismissDock],
  );

  useEffect(() => {
    followUpdates();
    void appVersion().then(setVersion).catch(() => undefined);
    void refreshV2();
    const timer = window.setInterval(() => void refreshV2(), REFRESH_MS);
    const offV2 = onV2Change(() => void refreshV2());
    const offState = onEvent("relay://state", setFlags);
    return () => {
      window.clearInterval(timer);
      offV2();
      offState();
    };
  }, [refreshV2]);

  useEffect(() => {
    if (flags.dnd) setPulsing(false);
  }, [flags.dnd]);

  // Native activation is verified in Rust before this event arrives. A refused activation leaves
  // the Dock clickable with a hint; an ended session hides it without clearing the draft.
  useEffect(() => {
    if (!inTauri) return;
    return onDockFocus((event) => {
      sessionRef.current = event.visible ? event.sessionId : null;
      setDockVisible(event.visible);
      setFocusHint(event.hint === "click-to-focus");
      setGrowing(false);
      if (!event.visible) {
        sentHeight.current = null;
        inputRef.current?.blur();
        return;
      }
      if (event.focused) requestAnimationFrame(() => inputRef.current?.focus());
      // Re-measure for this session: the draft may have grown while the Dock was hidden.
      void applyHeight(true);
    });
  }, [applyHeight]);

  useEffect(() => {
    if (!inTauri) return;
    return onDockDpi((event) => {
      if (sessionRef.current !== event.sessionId) return;
      // Rust has already re-anchored to the new work area; measure once the webview has reflowed.
      requestAnimationFrame(() => void applyHeight(true));
    });
  }, [applyHeight]);

  // The measurable content root only exists once the v2 read resolves, so the observer must attach
  // when that happens and not only on the first mount (which renders the loading tab instead).
  const contentReady = v2 !== null;

  useEffect(() => {
    const content = contentRef.current;
    if (!content || !inTauri) return;
    let frame = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => void applyHeight());
    });
    observer.observe(content);
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
    };
  }, [applyHeight, contentReady]);

  useEffect(() => {
    void applyHeight();
  }, [dockLayout, applyHeight, contentReady]);

  useEffect(() => {
    if (!inTauri) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || pickerOpenRef.current) return;
      event.preventDefault();
      void dismissDock("escape");
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [dismissDock]);

  /** Badge activation opens the waiting list from the unfiltered read. */
  const openWaiting = useCallback(() => {
    setShowWaiting(true);
  }, []);

  const openComposer = useCallback(() => {
    setComposing(true);
  }, []);

  const closeComposer = useCallback(() => {
    void dismissDock("escape");
  }, [dismissDock]);

  const toggleDnd = useCallback(async () => {
    const next = await setDnd(!flags.dnd).catch(() => null);
    if (next) setFlags(next);
  }, [flags.dnd]);

  const shownEdge: HoverEdge = "right";

  if (!v2) {
    // Nothing to count yet: a quiet tab that still places itself, so the window can be shown.
    return (
      <div
        className={`hroot${inTauri ? "" : " dev"}`}
        data-edge={shownEdge}
        data-mode="tab"
        data-visible={dockVisible ? "true" : "false"}
        data-growing="false"
      >
        <div ref={shell} className="tab" aria-label={failed ? "Relay: data unavailable" : "Relay: loading"}>
          <span className="cnt">{failed ? "!" : "·"}</span>
        </div>
      </div>
    );
  }

  const needsYou = needsYouCount(v2.tasks);
  const badge = needsYou > 0 ? String(needsYou) : update.kind === "ready" ? "↑" : null;
  const fit = update.kind !== "idle" && update.kind !== "checking";

  return (
    <div
      className={`hroot${inTauri ? "" : " dev"}`}
      data-edge={shownEdge}
      data-mode="open"
      data-visible={dockVisible ? "true" : "false"}
      data-growing={growing ? "true" : "false"}
      data-focus-hint={focusHint ? "click-to-focus" : undefined}
    >
      <div
        ref={shell}
        className={`hov dock${fit ? " fit" : ""}`}
        data-compose={composing ? "on" : undefined}
        data-dock-state={dockLayout}
      >
        <Panel
          dock={dock}
          draftMode={draft.mode}
          v2={v2}
          version={version}
          needsYou={needsYou}
          badge={badge}
          onOpenWaiting={openWaiting}
          showWaiting={showWaiting}
          onToggleWaiting={() => setShowWaiting((open) => !open)}
          pulse={pulse}
          pulsing={pulsing}
          onPulseEnd={() => setPulsing(false)}
          dockBrief={dockBrief}
          onDockBrief={setDockBrief}
          onDockChanged={() => void refreshV2()}
          flags={flags}
          update={update}
          failed={failed}
          onDnd={() => void toggleDnd()}
          composing={composing}
          onCompose={openComposer}
          onCloseComposer={closeComposer}
          onComposerActive={(active) => {
            composerActive.current = active;
          }}
        />
        {focusHint && (
          <div className="focus-hint" role="status">
            Click to focus
          </div>
        )}
      </div>
      {!inTauri && (
        <div className="devbar">
          <button type="button" className="btn" onClick={pulse}>
            Simulate change
          </button>
        </div>
      )}
    </div>
  );
}

interface PanelProps {
  dock: DockBridge;
  draftMode: V2AgentMode | null;
  v2: V2WorkspaceSnapshot;
  version: string;
  needsYou: number;
  badge: string | null;
  onOpenWaiting: () => void;
  showWaiting: boolean;
  onToggleWaiting: () => void;
  pulse: () => void;
  pulsing: boolean;
  onPulseEnd: () => void;
  dockBrief: string | null;
  onDockBrief: (id: string | null) => void;
  onDockChanged: () => void;
  flags: HoverFlags;
  update: UpdateState;
  failed: boolean;
  onDnd: () => void;
  composing: boolean;
  onCompose: () => void;
  onCloseComposer: () => void;
  onComposerActive: (active: boolean) => void;
}

function Panel({
  dock,
  draftMode,
  v2,
  version,
  needsYou,
  badge,
  onOpenWaiting,
  showWaiting,
  onToggleWaiting,
  pulsing,
  onPulseEnd,
  dockBrief,
  onDockBrief,
  onDockChanged,
  flags,
  update,
  failed,
  onDnd,
  composing,
  onCompose,
  onCloseComposer,
  onComposerActive,
}: PanelProps) {
  const waiting = waitingTasks(v2.tasks);
  const needs = waiting.length > 0;
  const openTask = (task: string) => void openMain(taskRoute(task));

  const needsSection = showWaiting ? (
    <WaitingList
      key="waiting"
      tasks={waiting}
      projects={v2.projects}
      dockBrief={dockBrief}
      onDockBrief={onDockBrief}
      onDockChanged={onDockChanged}
      openTask={openTask}
      onClose={onToggleWaiting}
    />
  ) : (
    <button
      type="button"
      className={`grp need-toggle${needs ? " has" : ""} ${pulsing ? "pulse" : ""}`}
      key="needs"
      onClick={needs ? onOpenWaiting : undefined}
      onAnimationEnd={onPulseEnd}
      aria-expanded={showWaiting}
      disabled={!needs}
    >
      {needs ? (
        <>
          <span className="wait-dot" />
          <b>{waiting.length}</b> {waiting.length === 1 ? "task needs you" : "tasks need you"}
          <Icon name="ext" small className="push" />
        </>
      ) : (
        <>
          <span className="ok">✓</span> Nothing needs you
        </>
      )}
    </button>
  );

  return (
    <DockContext.Provider value={dock}>
      <div className="dock-viewport" ref={dock.viewportRef}>
        <div className="dock-scroll">
          <div className="dock-content" ref={dock.contentRef} data-layout={dock.layout}>
            <div className="hh">
              <span className="wordmark" data-mode={draftMode ?? undefined}>
                relay<span className="dot" key={draftMode ?? "none"}>.</span>
              </span>
              {flags.paused && <span className="meta">capture paused</span>}
              <span className="push" />
              {badge && (
                <button
                  type="button"
                  className={`b${needsYou ? "" : " upd"}`}
                  aria-label={`${needsYou} tasks need you — show waiting tasks`}
                  onClick={onOpenWaiting}
                >
                  {badge}
                </button>
              )}
              <button type="button" className={`rb${composing ? " on" : ""}`} title="Capture a note" aria-pressed={composing} onClick={onCompose}>
                <Icon name="pencil" />
              </button>
              <button type="button" className={`rb${flags.dnd ? " on" : ""}`} title="Do not disturb" aria-pressed={flags.dnd} onClick={onDnd}>
                <Icon name="moon" />
              </button>
            </div>

            <UpdateBanner update={update} version={version} />

            <div className="hb">{needsSection}</div>

            {composing && (
              <section className="hcomposer" aria-label="Capture">
                <Composer compact autoFocus onActiveChange={onComposerActive} />
                <button type="button" className="btn ghost" onClick={onCloseComposer}>
                  Close composer
                </button>
              </section>
            )}

            <div className="hfoot">
              {failed ? (
                <div className="slot warn-fg">Could not read Relay's data · retrying</div>
              ) : null}
              <div className="hf">
                <kbd title="Summon">{SUMMON_CHORD}</kbd>
                <button type="button" className="btn ghost open" onClick={() => void showMainWindow()}>
                  <Icon name="ext" />
                  Open Relay
                </button>
              </div>
            </div>
          </div>
        </div>
      </div>
    </DockContext.Provider>
  );
}

/** The main window owns the updater; this banner mirrors it. Restart only installs what is already downloaded.
    "Up to date" and "checking" show nothing. */
function UpdateBanner({ update, version }: { update: UpdateState; version: string }) {
  if (update.kind === "ready") {
    return (
      <div className="ban">
        <span className="ic">
          <Icon name="download" />
        </span>
        <span className="tx">
          <b>Relay {update.version} is ready</b>
          <small>signed · restarts Relay</small>
        </span>
        <button type="button" className="btn pri" onClick={requestInstall}>
          Restart
        </button>
      </div>
    );
  }
  if (update.kind === "available" || update.kind === "downloading") {
    const pct = update.kind === "downloading" && update.progress !== null ? Math.round(update.progress * 100) : null;
    return (
      <div className="ban">
        <span className="ic">
          <Icon name="download" />
        </span>
        <span className="tx">
          <b>
            {update.kind === "available" ? `Relay ${update.version} available` : `Downloading ${update.version}`}
            {pct !== null && ` · ${pct}%`}
          </b>
          {pct !== null && (
            <span className="trk">
              <i style={{ width: `${pct}%` }} />
            </span>
          )}
        </span>
        <button type="button" className="btn" onClick={() => void openMain("/settings")}>
          Details
        </button>
      </div>
    );
  }
  if (update.kind === "error") {
    return (
      <div className="ban warn">
        <span className="ic">!</span>
        <span className="tx">
          <b>Update check failed</b>
          <small>Relay {version}</small>
        </span>
        <button type="button" className="btn" onClick={() => void requestUpdateCheck()}>
          Retry
        </button>
      </div>
    );
  }
  return null;
}

/** The full waiting list from the unfiltered v2 read, newest change first, inside Dock bounds. */
function WaitingList({
  tasks,
  projects,
  dockBrief,
  onDockBrief,
  onDockChanged,
  openTask,
  onClose,
}: {
  tasks: V2WorkspaceSnapshot["tasks"];
  projects: V2WorkspaceSnapshot["projects"];
  dockBrief: string | null;
  onDockBrief: (id: string | null) => void;
  onDockChanged: () => void;
  openTask: (id: string) => void;
  onClose: () => void;
}) {
  const briefed = tasks.find((task) => task.id === dockBrief);
  return (
    <section className="grp waiting" aria-label="Tasks waiting on you">
      <h3>
        Waiting on you <span className="meta">{tasks.length}</span>
        <button type="button" className="linkish push" onClick={onClose}>
          Hide
        </button>
      </h3>
      <div className="waiting-list">
        {tasks.map((task) => (
          <div className="wt-row" key={task.id}>
            <button
              type="button"
              className="wt"
              aria-expanded={
                (task.agentMode === "wait" || task.agentMode === "grill") && dockBrief === task.id
              }
              onClick={() =>
                task.agentMode === "wait" || task.agentMode === "grill"
                  ? onDockBrief(dockBrief === task.id ? null : task.id)
                  : openTask(task.id)
              }
            >
              <span className="t">{task.originalLine}</span>
              <span className="meta">{task.status.replace(/_/g, " ")}</span>
            </button>
            <button
              type="button"
              className="ib"
              title="Open in Relay"
              onClick={() => openTask(task.id)}
            >
              <Icon name="ext" small />
            </button>
          </div>
        ))}
      </div>
      {briefed && (briefed.agentMode === "wait" || briefed.agentMode === "grill") && (
        <div className="dock-brief" onClick={(event) => event.stopPropagation()}>
          {briefed.agentMode === "grill" ? (
            <GrillPanel compact taskId={briefed.id} projects={projects} onChanged={onDockChanged} />
          ) : (
            <BriefPanel
              compact
              taskId={briefed.id}
              projects={projects}
              onChanged={onDockChanged}
            />
          )}
        </div>
      )}
    </section>
  );
}
