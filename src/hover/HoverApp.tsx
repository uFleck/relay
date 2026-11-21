import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { EMPTY_DRAFT, type DraftSummary } from "../directives";
import { Composer } from "../components/Composer";
import { BriefPanel } from "../components/BriefPanel";
import { GrillPanel } from "../components/GrillPanel";
import { Icon, Mark } from "../components/Icon";
import {
  dockDismiss,
  dockReady,
  dockResize,
  getSnapshot,
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
  type V2WorkspaceSnapshot,
} from "../ipc";
import { claimState, clock, elapsed, liveClaims, nextDue, todayScope, until, useNow } from "../model";
import { taskRoute } from "../route";
import type { ClaimHolder, HoverEdge, HoverMode, Snapshot } from "../types";
import type { V2AgentMode } from "../ipc";
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

/* Rust does not send `relay://change` yet, so the hover also re-reads the snapshot on a slow timer. */
const REFRESH_MS = 20_000;

/** Browser dev only: `?calm` drops proposals and acks every change, for the idle panel. */
const calm = !inTauri && new URLSearchParams(window.location.search).has("calm");

function calmed(s: Snapshot): Snapshot {
  return {
    ...s,
    proposals: [],
    tasks: s.tasks.map((t) => ({
      ...t,
      changes: t.changes.map((c) => (c.kind === "requirement" ? { ...c, ackedAt: c.ackedAt ?? c.at, ackedBy: c.ackedBy ?? "you" } : c)),
    })),
  };
}

export function HoverApp() {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [v2, setV2] = useState<V2WorkspaceSnapshot | null>(null);
  const [failed, setFailed] = useState(false);
  // The Dock is always mounted so an unsent draft survives Esc, blur and a second summon. The
  // native window itself is hidden until the hotkey opens a session.
  const [mode, setMode] = useState<HoverMode>("open");
  const [flags, setFlags] = useState<HoverFlags>({ pinned: false, dnd: false, paused: false, ready: false, visible: false });
  const [pulsing, setPulsing] = useState(false);
  const [composing, setComposing] = useState(!calm);
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
  const collapseTimer = useRef<number | undefined>(undefined);
  const dragging = useRef(false);
  // Native session plumbing. Every queued measurement carries the session id it started with.
  const sessionRef = useRef<number | null>(null);
  const layoutRef = useRef(dockLayout);
  layoutRef.current = dockLayout;
  const pickerOpenRef = useRef(false);
  const sentHeight = useRef<number | null>(null);
  const readySent = useRef(false);
  // The pointer poll and the timers read these without restarting.
  const live = useRef({ mode, pinned: flags.pinned, dnd: flags.dnd, composer: false, waiting: false, brief: false });
  live.current = {
    mode,
    pinned: flags.pinned,
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

  const refresh = useCallback(async () => {
    try {
      const raw = await getSnapshot();
      setSnap(calm ? calmed(raw) : raw);
      setFailed(false);
    } catch (error) {
      console.error("[relay] hover snapshot", error);
      setFailed(true);
    }
  }, []);

  // The badge and the waiting list come from the same v2 read the main window uses. A new
  // waiting Task id pulses the tab; a committed write refreshes both surfaces.
  const refreshV2 = useCallback(async () => {
    try {
      const workspace = await v2WorkspaceSnapshot();
      const ids = new Set(waitingTasks(workspace.tasks).map((task) => task.id));
      if (waitingIds.current && [...ids].some((id) => !waitingIds.current!.has(id))) pulse();
      waitingIds.current = ids;
      setV2(workspace);
    } catch (error) {
      console.warn("[relay] hover v2 workspace", error);
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
    void refresh();
    void refreshV2();
    const timer = window.setInterval(() => {
      void refresh();
      void refreshV2();
    }, REFRESH_MS);
    const offChange = onEvent("relay://change", () => void refresh());
    const offV2 = onV2Change(() => void refreshV2());
    const offState = onEvent("relay://state", setFlags);
    return () => {
      window.clearInterval(timer);
      offChange();
      offV2();
      offState();
    };
  }, [refresh, refreshV2]);

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

  const scheduleCollapse = useCallback(() => {
    if (
      live.current.pinned || dragging.current || composerActive.current || live.current.waiting ||
      live.current.brief || collapseTimer.current !== undefined
    ) return;
    collapseTimer.current = window.setTimeout(() => {
      collapseTimer.current = undefined;
      if (live.current.pinned || composerActive.current || live.current.waiting || live.current.brief) return;
      setMode("tab");
      setComposing(false);
      void hoverFocus(false);
    }, COLLAPSE_GRACE_MS);
  }, []);

  // The measurable content root only exists once both reads resolve, so the observer must attach
  // when that happens and not only on the first mount (which renders the loading tab instead).
  const contentReady = snap !== null && v2 !== null;

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
    setMode("open");
    setShowWaiting(true);
  }, []);

  const openComposer = useCallback(() => {
    setMode("open");
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

  if (!snap || !v2) {
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

  const scope = todayScope(snap);
  const needsYou = needsYouCount(v2.tasks);
  const badge = needsYou > 0 ? String(needsYou) : update.kind === "ready" ? "↑" : null;
  const fit = update.kind !== "idle" && update.kind !== "checking";

  return (
    <div
      className={`hroot${inTauri ? "" : " dev"}`}
      data-edge={shownEdge}
      data-mode={mode}
      data-visible={dockVisible ? "true" : "false"}
      data-growing={growing ? "true" : "false"}
      data-focus-hint={focusHint ? "click-to-focus" : undefined}
    >
      {mode === "tab" ? (
        <div
          ref={shell}
          className={`tab${pulsing ? " pulse" : ""}`}
          onMouseEnter={inTauri ? undefined : () => setMode("open")}
          onAnimationEnd={() => setPulsing(false)}
          aria-label={`Relay: ${scope.total} today, ${scope.inProgress} in progress, ${needsYou} need you`}
        >
          {badge && (
            <button
              type="button"
              className={`b${needsYou ? "" : " upd"}`}
              aria-label={`${needsYou} tasks need you — show waiting tasks`}
              onClick={(event) => {
                event.stopPropagation();
                openWaiting();
              }}
            >
              {badge}
            </button>
          )}
          <span className="cnt">{scope.total}</span>
          <span className="fill">
            <i style={{ height: `${scope.total ? (scope.inProgress / scope.total) * 100 : 0}%` }} />
          </span>
          <span className="pips">
            {snap.sync
              .filter((s) => s.state !== "off")
              .map((s) => (
                <i key={s.name} className={`dot sync-${s.state}`} />
              ))}
          </span>
        </div>
      ) : (
        <div
          ref={shell}
          className={`hov dock${fit ? " fit" : ""}`}
          data-compose={composing ? "on" : undefined}
          data-dock-state={dockLayout}
        >
          <Panel
            dock={dock}
            draftMode={draft.mode}
            snap={snap}
            v2={v2}
            showWaiting={showWaiting}
            onToggleWaiting={() => setShowWaiting((open) => !open)}
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
      )}
      {!inTauri && (
        <div className="devbar">
          <button type="button" className="btn" onClick={pulse}>
            Simulate change
          </button>
          <button type="button" className="btn" onClick={() => setMode((m) => (m === "tab" ? "open" : "tab"))}>
            {mode === "tab" ? "Open" : "Collapse"}
          </button>
        </div>
      )}
    </div>
  );
}

interface PanelProps {
  snap: Snapshot;
  v2: V2WorkspaceSnapshot;
  showWaiting: boolean;
  onToggleWaiting: () => void;
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
  dock: DockBridge;
  draftMode: V2AgentMode | null;
}

function Panel({
  snap,
  v2,
  showWaiting,
  onToggleWaiting,
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
  dock,
  draftMode,
}: PanelProps) {
  const now = useNow();
  const scope = todayScope(snap);
  const waiting = waitingTasks(v2.tasks);
  const needs = waiting.length > 0;
  const due = nextDue(snap, now);
  const claims = liveClaims(snap.tasks);
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
      className={`grp need-toggle${needs ? " has" : ""}`}
      key="needs"
      onClick={onToggleWaiting}
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

  const claimsSection = (
    <section className="grp" key="claims">
      <h3>
        Agents working {claims.length > 0 && <span className="meta">lease {claims[0].claim.leaseMinutes} min</span>}
      </h3>
      {claims.map((t) => {
        const c = t.claim;
        const state = claimState(c, now);
        const leaseLeft = Math.max(0.04, 1 - (now - Date.parse(c.lastProgressAt)) / (c.leaseMinutes * 60_000));
        return (
          <button type="button" className="cl" key={c.id} onClick={() => openTask(t.id)}>
            <Avatar holder={c.holder} stale={state === "stale"} lease={leaseLeft} />
            <span className="t">{t.title}</span>
            <span className="el">
              {c.holder} · {elapsed(c.claimedAt, now)}
            </span>
            <span className="p">
              {c.lastProgress} ·{" "}
              <span className={state === "stale" ? "stale" : undefined}>{elapsed(c.lastProgressAt, now)} ago</span>
            </span>
          </button>
        );
      })}
      {claims.length === 0 && <div className="calm">No agent holds a claim.</div>}
    </section>
  );

  const dueSection = due ? (
    <button type="button" className="nx" key="due" onClick={() => openTask(due.id)}>
      <span className="tm">{clock(due.due!.at)}</span>
      <span className="t">{due.title}</span>
      <span className="when">{until(due.due!.at, now)}</span>
    </button>
  ) : (
    <div className="calm" key="due">
      <Icon name="clock" className="muted-fg" />
      Nothing with a time due today
    </div>
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
        <button type="button" className={`rb${composing ? " on" : ""}`} title="Capture a note" aria-pressed={composing} onClick={onCompose}>
          <Icon name="pencil" />
        </button>
        <button type="button" className={`rb${flags.dnd ? " on" : ""}`} title="Do not disturb" aria-pressed={flags.dnd} onClick={onDnd}>
          <Icon name="moon" />
        </button>
        </div>

      <UpdateBanner update={update} version={snap.version} />

      <div className="hero">
        <span className="big">
          {scope.total}
          <small>today</small>
        </span>
        <span className="chips">
          <span className="chip" style={{ "--c": "var(--c-active)" } as React.CSSProperties}>
            <i />
            {scope.inProgress} in progress
          </span>
          <span className="chip" style={{ "--c": "var(--c-blocked)" } as React.CSSProperties}>
            <i />
            {scope.blocked} blocked
          </span>
          {needs && (
            <button
              type="button"
              className="chip need"
              style={{ "--c": "var(--warn)" } as React.CSSProperties}
              onClick={onToggleWaiting}
              aria-expanded={showWaiting}
              aria-label={`${waiting.length} tasks need you — show waiting tasks`}
            >
              <i />
              {waiting.length} need you
            </button>
          )}
        </span>
      </div>

      <div className="hb">{needs ? [needsSection, claimsSection] : [claimsSection, dueSection, needsSection]}</div>

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
        ) : (
          needs && due && <div className="slot">Next due {clock(due.due!.at)} · {due.title}</div>
        )}
        <div className="hf">
          <span className="syncs">
            {snap.sync.every((s) => s.state === "off") && <span>no sources connected</span>}
            {snap.sync
              .filter((s) => s.state !== "off")
              .map((s) => (
                <span key={s.name} title={s.reason ? `${s.name}: ${s.reason}` : `${s.name}: ok`}>
                  <i className={`dot sync-${s.state}`} />
                  {s.name}
                </span>
              ))}
          </span>
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

const AVATAR: Record<ClaimHolder, { initials: string; color: string }> = {
  T3: { initials: "T3", color: "var(--i1)" },
  OpenCode: { initials: "OC", color: "var(--s3)" },
  "Claude Code": { initials: "CC", color: "var(--i3)" },
};

/** The agent's initials; the ring is the lease left since its last progress. */
function Avatar({ holder, stale, lease }: { holder: ClaimHolder; stale: boolean; lease: number }) {
  const a = AVATAR[holder];
  const c = 2 * Math.PI * 16;
  return (
    <span className={`av${stale ? " stale" : ""}`} style={{ "--av": a.color } as React.CSSProperties}>
      <svg viewBox="0 0 36 36" aria-hidden>
        <circle className="bg" cx="18" cy="18" r="16" />
        <circle className="fg" cx="18" cy="18" r="16" strokeDasharray={`${c * lease} ${c}`} />
      </svg>
      {a.initials}
      <span className="st" />
    </span>
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
