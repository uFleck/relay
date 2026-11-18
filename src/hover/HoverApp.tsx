import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Icon, Mark } from "../components/Icon";
import { ProposalCard } from "../components/ProposalCard";
import {
  getSnapshot,
  hoverDrag,
  hoverPlace,
  hoverPointer,
  hoverReady,
  hoverSetPinned,
  inTauri,
  onEvent,
  openMain,
  requestUpdateCheck,
  setDnd,
  showMainWindow,
  type HoverFlags,
} from "../ipc";
import { claimState, clock, elapsed, isUnacked, liveClaims, nextDue, todayScope, until, useNow } from "../model";
import { proposalRoute, taskRoute } from "../route";
import type { HoverEdge, HoverMode, Snapshot } from "../types";
import { followUpdates, requestInstall, useUpdater, type UpdateState } from "../updater";
import { initialPlacement, placeIn, placementAfterDrag, type Anchor, type Screen } from "./place";

const COLLAPSE_GRACE_MS = 400;
const POINTER_POLL_MS = 50;
/* Rust does not send `relay://change` yet, so the hover also re-reads the snapshot on a slow timer. */
const REFRESH_MS = 20_000;
const MAX_NEEDS_CARDS = 2;

/** What needs the user: open proposals and unacked requirements. A new id is what makes the tab pulse. */
function needIds(s: Snapshot) {
  return new Set([
    ...s.proposals.map((p) => p.id),
    ...s.tasks.flatMap((t) => t.changes.filter(isUnacked).map((c) => c.id)),
  ]);
}

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
  const [failed, setFailed] = useState(false);
  const [mode, setMode] = useState<HoverMode>("tab");
  const [flags, setFlags] = useState<HoverFlags>({ pinned: false, dnd: false, paused: false, ready: false, visible: false });
  const [placement, setPlacement] = useState<{ screen: Screen; anchor: Anchor } | null>(null);
  const [pulsing, setPulsing] = useState(false);
  const update = useUpdater();
  const shell = useRef<HTMLDivElement>(null);
  const seen = useRef<Set<string> | null>(null);
  const collapseTimer = useRef<number | undefined>(undefined);
  const dragging = useRef(false);
  // The pointer poll and the timers read these without restarting.
  const live = useRef({ mode, pinned: flags.pinned, dnd: flags.dnd });
  live.current = { mode, pinned: flags.pinned, dnd: flags.dnd };

  const pulse = useCallback(() => {
    if (live.current.dnd) return;
    // Remove then re-add the class so a change mid-pulse restarts the 3 beats.
    setPulsing(false);
    requestAnimationFrame(() => setPulsing(true));
  }, []);

  const refresh = useCallback(async () => {
    try {
      const raw = await getSnapshot();
      const s = calm ? calmed(raw) : raw;
      const ids = needIds(s);
      if (seen.current && [...ids].some((id) => !seen.current!.has(id))) pulse();
      seen.current = ids;
      setSnap(s);
      setFailed(false);
    } catch (error) {
      console.error("[relay] hover snapshot", error);
      setFailed(true);
    }
  }, [pulse]);

  useEffect(() => {
    followUpdates();
    void refresh();
    const timer = window.setInterval(() => void refresh(), REFRESH_MS);
    const offChange = onEvent("relay://change", () => void refresh());
    const offState = onEvent("relay://state", setFlags);
    return () => {
      window.clearInterval(timer);
      offChange();
      offState();
    };
  }, [refresh]);

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
    if (live.current.pinned || dragging.current || collapseTimer.current !== undefined) return;
    collapseTimer.current = window.setTimeout(() => {
      collapseTimer.current = undefined;
      if (!live.current.pinned) setMode("tab");
    }, COLLAPSE_GRACE_MS);
  }, []);

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

  if (!snap) {
    // Nothing to count yet: a quiet tab that still places itself, so the window can be shown.
    return (
      <div className={`hroot${inTauri ? "" : " dev"}`} data-edge={shownEdge} data-mode="tab">
        <div ref={shell} className="tab" aria-label={failed ? "Relay: data unavailable" : "Relay: loading"}>
          <Mark size={22} />
          <span className="cnt">{failed ? "!" : "·"}</span>
        </div>
      </div>
    );
  }

  const scope = todayScope(snap);
  const badge = scope.needsYou > 0 ? String(scope.needsYou) : update.kind === "ready" ? "↑" : null;
  const fit = update.kind !== "idle" && update.kind !== "checking";

  return (
    <div className={`hroot${inTauri ? "" : " dev"}`} data-edge={shownEdge} data-mode={mode}>
      {mode === "tab" ? (
        <div
          ref={shell}
          className={`tab${pulsing ? " pulse" : ""}`}
          onMouseEnter={inTauri ? undefined : expand}
          onAnimationEnd={() => setPulsing(false)}
          aria-label={`Relay: ${scope.total} today, ${scope.needsYou} need you`}
        >
          <Mark size={22} />
          {badge && <span className={`b${scope.needsYou ? "" : " upd"}`}>{badge}</span>}
          <span className="cnt">{scope.total}</span>
        </div>
      ) : (
        <div
          ref={shell}
          className={`hov${fit ? " fit" : ""}`}
          onMouseEnter={inTauri ? undefined : expand}
          onMouseLeave={inTauri ? undefined : scheduleCollapse}
        >
          <Panel
            snap={snap}
            flags={flags}
            update={update}
            failed={failed}
            onPin={() => void togglePin()}
            onDnd={() => void toggleDnd()}
            onDrag={() => void drag()}
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
  flags: HoverFlags;
  update: UpdateState;
  failed: boolean;
  onPin: () => void;
  onDnd: () => void;
  onDrag: () => void;
}

function Panel({ snap, flags, update, failed, onPin, onDnd, onDrag }: PanelProps) {
  const now = useNow();
  const scope = todayScope(snap);
  const needs = scope.needsYou > 0;
  const due = nextDue(snap, now);
  const claims = liveClaims(snap.tasks);
  const openTask = (task: string) => void openMain(taskRoute(task));

  // Drag from the header or grip, but not from its buttons.
  const startDrag = (e: React.PointerEvent) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
    e.preventDefault();
    onDrag();
  };

  const needsSection = (
    <section className="sec" key="needs">
      <h3>Needs you {needs && <span className="meta">{scope.needsYou}</span>}</h3>
      {needs ? (
        <NeedsYou snap={snap} openTask={openTask} />
      ) : (
        <div className="calm">
          Nothing waiting.
          {snap.lastProposalHandledAt && ` Last proposal handled ${clock(snap.lastProposalHandledAt)}.`}
        </div>
      )}
    </section>
  );

  const claimsSection = (
    <section className="sec" key="claims">
      <h3>
        Active claims {claims.length > 0 && <span className="meta">lease {claims[0].claim.leaseMinutes}m</span>}
      </h3>
      {claims.map((t) => {
        const c = t.claim;
        const state = claimState(c, now);
        const leaseUsed = Math.min(100, ((now - Date.parse(c.lastProgressAt)) / (c.leaseMinutes * 60_000)) * 100);
        return (
          <button type="button" className="cl" key={c.id} onClick={() => openTask(t.id)}>
            <span className={`live ${state}`} />
            <span className="t">{t.title}</span>
            <span className="el">
              {c.holder} · {elapsed(c.claimedAt, now)}
            </span>
            <span className="p">
              {c.lastProgress}
              <span className={`meta${state === "stale" ? " stale" : ""}`}>{elapsed(c.lastProgressAt, now)} ago</span>
            </span>
            {!needs && (
              <span className="bar">
                <i className={state === "stale" ? "st" : undefined} style={{ width: `${Math.max(3, leaseUsed)}%` }} />
              </span>
            )}
          </button>
        );
      })}
      {claims.length === 0 && <div className="calm">No agent holds a claim.</div>}
    </section>
  );

  const dueSection = (
    <section className="sec" key="due">
      <h3>Next due</h3>
      {due ? (
        <button type="button" className="nx" onClick={() => openTask(due.id)}>
          <Icon name="clock" className="muted-fg" />
          <span className="t">{due.title}</span>
          <span className="when">
            {clock(due.due!.at)} · {until(due.due!.at, now)}
          </span>
        </button>
      ) : (
        <div className="calm">Nothing with a time due today.</div>
      )}
    </section>
  );

  return (
    <>
      <div className="hh" onPointerDown={startDrag}>
        <Mark size={20} />
        <b>Relay</b>
        <span className={`need${needs ? "" : " zero"}`}>{scope.needsYou} needs you</span>
        {flags.paused && <span className="meta">capture paused</span>}
        <span className="push" />
        <button type="button" className={`ib${flags.dnd ? " on" : ""}`} title="Do not disturb" aria-pressed={flags.dnd} onClick={onDnd}>
          <Icon name="moon" />
        </button>
        <button type="button" className={`ib${flags.pinned ? " on" : ""}`} title="Pin open" aria-pressed={flags.pinned} onClick={onPin}>
          <Icon name="pin" />
        </button>
        <span className="ib grip" title="Drag to another edge">
          <Icon name="grip" />
        </span>
      </div>

      <div className="gauge">
        <div className="counts">
          <b>{scope.total}</b>today<span className="sep">/</span>
          <b>{scope.inProgress}</b>in progress<span className="sep">/</span>
          <span className="bl">
            <b>{scope.blocked}</b>blocked
          </span>
        </div>
        <div className="syncs">
          {snap.sync.length === 0 && <span className="meta">no sources yet</span>}
          {snap.sync.map((s) => (
            <span key={s.name} title={s.reason ? `${s.name}: ${s.reason}` : `${s.name}: ok`}>
              <i className={`dot sync-${s.state}`} />
              {s.name}
            </span>
          ))}
        </div>
      </div>

      <div className="hb">{needs ? [needsSection, claimsSection] : [claimsSection, dueSection, needsSection]}</div>

      <div className="hfoot">
        <UpdateRow update={update} version={snap.version} failed={failed} slot={needs && due ? `next due · ${due.title} · ${clock(due.due!.at)}` : null} />
        <div className="hf">
          <span className="meta">
            <kbd>Ctrl+Alt+R</kbd> summon
          </span>
          <button type="button" className="btn ghost open" onClick={() => void showMainWindow()}>
            <Icon name="ext" />
            Open Relay
          </button>
        </div>
      </div>
    </>
  );
}

/** The main window owns the updater; this row mirrors it. Restart only installs what is already downloaded. */
function UpdateRow({ update, version, failed, slot }: { update: UpdateState; version: string; failed: boolean; slot: string | null }) {
  if (update.kind === "ready") {
    return (
      <div className="up ready">
        <Icon name="download" className="accent-fg" />
        <span>
          <b>{update.version}</b> ready <span className="meta">· signed</span>
        </span>
        <button type="button" className="btn pri" onClick={requestInstall}>
          Restart
        </button>
      </div>
    );
  }
  if (update.kind === "available" || update.kind === "downloading") {
    return (
      <div className="up ready">
        <Icon name="download" className="accent-fg" />
        <span>
          <b>{update.version}</b>{" "}
          {update.kind === "available"
            ? "available"
            : `downloading${update.progress === null ? "…" : ` ${Math.round(update.progress * 100)}%`}`}
        </span>
        <button type="button" className="btn" onClick={() => void openMain("/settings")}>
          Details
        </button>
      </div>
    );
  }
  if (failed) return <div className="up idle warn-fg">Could not read Relay's data · retrying</div>;
  if (slot) return <div className="up idle">{slot}</div>;
  if (update.kind === "error") {
    return (
      <div className="up idle">
        Relay {version} · update check failed{" "}
        <button type="button" className="linkish" onClick={() => void requestUpdateCheck()}>
          retry
        </button>
      </div>
    );
  }
  return <div className="up idle">Relay {version} · {update.kind === "checking" ? "checking for updates…" : "up to date"}</div>;
}

/** Proposals first, then unacked changes; at most two cards, the rest count toward Review. */
function NeedsYou({ snap, openTask }: { snap: Snapshot; openTask: (id: string) => void }) {
  const scope = todayScope(snap);
  const items = [
    ...snap.proposals.map((p) => ({ type: "proposal" as const, p })),
    ...scope.unacked.map((u) => ({ type: "change" as const, ...u })),
  ];
  const shown = items.slice(0, MAX_NEEDS_CARDS);
  const more = items.length - shown.length;

  return (
    <>
      {shown.map((item) =>
        item.type === "proposal" ? (
          <ProposalCard
            key={item.p.id}
            size="compact"
            proposal={item.p}
            task={snap.tasks.find((t) => t.id === item.p.taskId)}
            onReview={() => void openMain(proposalRoute(item.p.id))}
          />
        ) : (
          item.change.kind === "requirement" && (
            <button type="button" className="card" key={item.change.id} onClick={() => openTask(item.task.id)}>
              <div className="hd">
                <Icon name="slack" className="muted-fg" />
                <b>{item.task.title}</b>
                <span className="meta">
                  {item.change.author.split(" ")[0]} · {clock(item.change.at)} · {item.change.confidence.toFixed(2)}
                </span>
              </div>
              <q>“{item.change.text}”</q>
              <div className="meta warn-fg">
                unacked
                {!item.task.claim || item.task.claim.releasedAt
                  ? " · no agent holds this task"
                  : Date.parse(item.task.claim.lastProgressAt) < Date.parse(item.change.at)
                    ? ` · ${item.task.claim.holder} claim hasn't checked in since`
                    : ` · ${item.task.claim.holder} claim checked in without acking`}
              </div>
            </button>
          )
        ),
      )}
      {more > 0 && (
        <button type="button" className="more" onClick={() => void openMain(snap.proposals.length ? "/review" : "/today")}>
          +{more} more in {snap.proposals.length ? "Review" : "Today"}
        </button>
      )}
    </>
  );
}
