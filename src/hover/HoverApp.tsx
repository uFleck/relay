import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Composer } from "../components/Composer";
import { Icon, Mark } from "../components/Icon";
import {
  getSnapshot,
  hoverDrag,
  hoverFocus,
  hoverPlace,
  hoverPointer,
  hoverReady,
  hoverSetPinned,
  inTauri,
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
import { followUpdates, requestInstall, useUpdater, type UpdateState } from "../updater";
import { needsYouCount, waitingTasks } from "../workspace";
import { initialPlacement, placeIn, placementAfterDrag, type Anchor, type Screen } from "./place";

const COLLAPSE_GRACE_MS = 400;
const POINTER_POLL_MS = 50;
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
  const [mode, setMode] = useState<HoverMode>("tab");
  const [flags, setFlags] = useState<HoverFlags>({ pinned: false, dnd: false, paused: false, ready: false, visible: false });
  const [placement, setPlacement] = useState<{ screen: Screen; anchor: Anchor } | null>(null);
  const [pulsing, setPulsing] = useState(false);
  const [composing, setComposing] = useState(false);
  const [showWaiting, setShowWaiting] = useState(false);
  const composerActive = useRef(false);
  const update = useUpdater();
  const shell = useRef<HTMLDivElement>(null);
  const waitingIds = useRef<Set<string> | null>(null);
  const collapseTimer = useRef<number | undefined>(undefined);
  const dragging = useRef(false);
  // The pointer poll and the timers read these without restarting.
  const live = useRef({ mode, pinned: flags.pinned, dnd: flags.dnd, composer: false, waiting: false });
  live.current = {
    mode,
    pinned: flags.pinned,
    dnd: flags.dnd,
    composer: composerActive.current,
    waiting: showWaiting,
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
    if (!inTauri) return;
    initialPlacement()
      .then(setPlacement)
      .catch((e) => console.error("[relay] hover placement", e));
  }, []);

  useEffect(() => {
    if (flags.dnd) setPulsing(false);
  }, [flags.dnd]);

  // Rust sizes the window, not the DOM: after each layout change, place the window around what is drawn.
  // The first placement also tells Rust the page is ready to be shown.
  useLayoutEffect(() => {
    const el = shell.current;
    if (!el || !placement) return;
    const { screen, anchor } = placement;
    const rect = placeIn(screen.area, anchor, el.offsetWidth * screen.scale, el.offsetHeight * screen.scale);
    void hoverPlace(rect)
      .then(() => (flags.ready ? undefined : hoverReady().then(setFlags)))
      .catch((e) => console.warn("[relay] hover_place failed", e));
  }, [mode, placement, update.kind, snap === null, failed, flags.ready]);

  const expand = useCallback(() => {
    window.clearTimeout(collapseTimer.current);
    collapseTimer.current = undefined;
    setMode("open");
  }, []);

  const scheduleCollapse = useCallback(() => {
    if (
      live.current.pinned || dragging.current || composerActive.current || live.current.waiting ||
      collapseTimer.current !== undefined
    ) return;
    collapseTimer.current = window.setTimeout(() => {
      collapseTimer.current = undefined;
      if (live.current.pinned || composerActive.current || live.current.waiting) return;
      setMode("tab");
      setComposing(false);
      void hoverFocus(false);
    }, COLLAPSE_GRACE_MS);
  }, []);

  /** Badge activation expands the Dock and opens the waiting list from the unfiltered read. */
  const openWaiting = useCallback(() => {
    window.clearTimeout(collapseTimer.current);
    collapseTimer.current = undefined;
    setMode("open");
    setShowWaiting(true);
  }, []);

  // A click-to-compose control takes keyboard focus for the shared Composer. While it is
  // focused or saving, the panel stays open and the hotkey keeps summoning the main window.
  const openComposer = useCallback(() => {
    window.clearTimeout(collapseTimer.current);
    collapseTimer.current = undefined;
    setMode("open");
    setComposing(true);
    void hoverFocus(true);
  }, []);

  const closeComposer = useCallback(() => {
    setComposing(false);
    composerActive.current = false;
    void hoverFocus(false);
    scheduleCollapse();
  }, [scheduleCollapse]);

  // The webview misreports hover once focus has moved elsewhere, so in the app the page asks Rust where
  // the cursor is. Only while the window is on screen.
  useEffect(() => {
    if (!inTauri || !flags.visible) return;
    let asking = false;
    const timer = window.setInterval(() => {
      if (asking || dragging.current) return;
      asking = true;
      hoverPointer()
        .then((at) => (at ? expand() : live.current.mode === "open" && scheduleCollapse()))
        .catch(() => undefined)
        .finally(() => (asking = false));
    }, POINTER_POLL_MS);
    return () => window.clearInterval(timer);
  }, [flags.visible, expand, scheduleCollapse]);

  const drag = useCallback(async () => {
    if (!inTauri) return;
    dragging.current = true;
    window.clearTimeout(collapseTimer.current);
    collapseTimer.current = undefined;
    try {
      await hoverDrag();
      const next = await placementAfterDrag();
      if (next) setPlacement(next);
    } catch (error) {
      console.warn("[relay] hover drag failed", error);
    } finally {
      dragging.current = false;
    }
  }, []);

  const togglePin = useCallback(async () => {
    const next = await hoverSetPinned(!flags.pinned).catch(() => null);
    if (next) setFlags(next);
    if (next && !next.pinned) scheduleCollapse();
  }, [flags.pinned, scheduleCollapse]);

  const toggleDnd = useCallback(async () => {
    const next = await setDnd(!flags.dnd).catch(() => null);
    if (next) setFlags(next);
  }, [flags.dnd]);

  const edge: HoverEdge = placement?.anchor.edge ?? "right";
  const devEdge = useState<HoverEdge>("right");
  const shownEdge = inTauri ? edge : devEdge[0];

  if (!snap || !v2) {
    // Nothing to count yet: a quiet tab that still places itself, so the window can be shown.
    return (
      <div className={`hroot${inTauri ? "" : " dev"}`} data-edge={shownEdge} data-mode="tab">
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
    <div className={`hroot${inTauri ? "" : " dev"}`} data-edge={shownEdge} data-mode={mode}>
      {mode === "tab" ? (
        <div
          ref={shell}
          className={`tab${pulsing ? " pulse" : ""}`}
          onMouseEnter={inTauri ? undefined : expand}
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
          className={`hov${fit ? " fit" : ""}`}
          data-compose={composing ? "on" : undefined}
          onMouseEnter={inTauri ? undefined : expand}
          onMouseLeave={inTauri ? undefined : scheduleCollapse}
        >
          <Panel
            snap={snap}
            v2={v2}
            showWaiting={showWaiting}
            onToggleWaiting={() => setShowWaiting((open) => !open)}
            flags={flags}
            update={update}
            failed={failed}
            onPin={() => void togglePin()}
            onDnd={() => void toggleDnd()}
            onDrag={() => void drag()}
            composing={composing}
            onCompose={openComposer}
            onCloseComposer={closeComposer}
            onComposerActive={(active) => {
              composerActive.current = active;
              if (active) {
                window.clearTimeout(collapseTimer.current);
                collapseTimer.current = undefined;
              } else {
                scheduleCollapse();
              }
            }}
          />
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
          <button type="button" className="btn" onClick={() => devEdge[1]((e) => (e === "right" ? "left" : "right"))}>
            Edge: {shownEdge}
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
  flags: HoverFlags;
  update: UpdateState;
  failed: boolean;
  onPin: () => void;
  onDnd: () => void;
  onDrag: () => void;
  composing: boolean;
  onCompose: () => void;
  onCloseComposer: () => void;
  onComposerActive: (active: boolean) => void;
}

function Panel({
  snap,
  v2,
  showWaiting,
  onToggleWaiting,
  flags,
  update,
  failed,
  onPin,
  onDnd,
  onDrag,
  composing,
  onCompose,
  onCloseComposer,
  onComposerActive,
}: PanelProps) {
  const now = useNow();
  const scope = todayScope(snap);
  const waiting = waitingTasks(v2.tasks);
  const needs = waiting.length > 0;
  const due = nextDue(snap, now);
  const claims = liveClaims(snap.tasks);
  const openTask = (task: string) => void openMain(taskRoute(task));

  // Drag from the header or grip, but not from its buttons.
  const startDrag = (e: React.PointerEvent) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
    e.preventDefault();
    onDrag();
  };

  const needsSection = showWaiting ? (
    <WaitingList key="waiting" tasks={waiting} openTask={openTask} onClose={onToggleWaiting} />
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
    <>
      <div className="hh" onPointerDown={startDrag}>
        <Mark size={20} />
        <b>Relay</b>
        {flags.paused && <span className="meta">capture paused</span>}
        <span className="push" />
        <button type="button" className={`rb${composing ? " on" : ""}`} title="Capture a note" aria-pressed={composing} onClick={onCompose}>
          <Icon name="pencil" />
        </button>
        <button type="button" className={`rb${flags.dnd ? " on" : ""}`} title="Do not disturb" aria-pressed={flags.dnd} onClick={onDnd}>
          <Icon name="moon" />
        </button>
        <button type="button" className={`rb${flags.pinned ? " on" : ""}`} title="Pin open" aria-pressed={flags.pinned} onClick={onPin}>
          <Icon name="pin" />
        </button>
        <span className="rb grip" title="Drag to another edge">
          <Icon name="grip" />
        </span>
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
          <kbd title="Summon">Ctrl Alt R</kbd>
          <button type="button" className="btn ghost open" onClick={() => void showMainWindow()}>
            <Icon name="ext" />
            Open Relay
          </button>
        </div>
      </div>
    </>
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
  openTask,
  onClose,
}: {
  tasks: V2WorkspaceSnapshot["tasks"];
  openTask: (id: string) => void;
  onClose: () => void;
}) {
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
          <button type="button" className="wt" key={task.id} onClick={() => openTask(task.id)}>
            <span className="t">{task.originalLine}</span>
            <span className="meta">{task.status.replace(/_/g, " ")}</span>
            <Icon name="ext" small />
          </button>
        ))}
      </div>
    </section>
  );
}
