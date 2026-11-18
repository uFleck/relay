import { useCallback, useEffect, useRef, useState } from "react";
import { ProposalCard } from "./components/ProposalCard";
import { Rail } from "./app/Rail";
import { TaskSheet } from "./app/TaskSheet";
import { TitleBar } from "./app/TitleBar";
import { AgentStrip, TaskGroups, TodayHeader } from "./app/Today";
import { EpisodeSheet, InboxView } from "./app/Slack";
import { AgentsView, ProjectsView, ReviewView, SettingsView, UpdateNotice } from "./app/Views";
import {
  ackChange,
  decideProposal,
  emitChange,
  errorText,
  getSnapshot,
  hoverState,
  inTauri,
  mcpStatus,
  onEvent,
  onMcpStatus,
  openExternal,
  setDnd,
  setPaused,
  slackStatus,
  type HoverFlags,
  type McpStatus,
  type SlackStatus,
} from "./ipc";
import { liveClaims, todayScope } from "./model";
import { parseRoute, readLocation, writeLocation, type Route, type View } from "./route";
import type { Change, Proposal, ProposalDecision, Snapshot } from "./types";
import { startUpdateChecks, useUpdater } from "./updater";

const typing = (el: EventTarget | null) =>
  el instanceof HTMLElement && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName));

/** Adds or removes `id` from a set held in state, returning a new set. */
const toggled = (set: ReadonlySet<string>, id: string, on: boolean) => {
  const next = new Set(set);
  if (on) next.add(id);
  else next.delete(id);
  return next;
};

export default function App() {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [route, setRoute] = useState<Route>(readLocation);
  const [query, setQuery] = useState("");
  const [deciding, setDeciding] = useState<ReadonlySet<string>>(new Set());
  const [acking, setAcking] = useState<ReadonlySet<string>>(new Set());
  const [notice, setNotice] = useState<string | null>(null);
  const [flags, setFlags] = useState<HoverFlags | null>(null);
  const [mcp, setMcp] = useState<McpStatus | null>(null);
  const [slack, setSlack] = useState<SlackStatus | null>(null);
  const update = useUpdater();
  const searchRef = useRef<HTMLInputElement>(null);
  /** Where focus was when the sheet opened, so Esc can put it back. */
  const returnFocus = useRef<HTMLElement | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSnap(await getSnapshot());
      setLoadError(null);
    } catch (error) {
      console.error("[relay] snapshot", error);
      setLoadError(errorText(error));
    }
  }, []);

  const refreshMcp = useCallback(async () => {
    try {
      setMcp(await mcpStatus());
    } catch (error) {
      console.warn("[relay] mcp_status", error);
    }
  }, []);

  const refreshSlack = useCallback(async () => {
    try {
      setSlack(await slackStatus());
    } catch (error) {
      console.warn("[relay] slack_status", error);
    }
  }, []);

  const navigate = useCallback((next: Route) => {
    writeLocation(next);
    setRoute(next);
  }, []);

  useEffect(() => {
    startUpdateChecks();
    void refresh();
    // Nothing in Rust pushes data changes yet, so coming back to the window re-reads the snapshot.
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    const offChange = onEvent("relay://change", () => {
      void refresh();
      void refreshSlack();
    });
    const offOpen = onEvent("relay://open", (r) => navigate(parseRoute(r)));
    const offState = onEvent("relay://state", setFlags);
    if (inTauri) hoverState().then(setFlags, (e) => console.warn("[relay] hover_state", e));
    // Subscribe before asking, so a status Rust sends in between is not lost.
    const offMcp = onMcpStatus(setMcp);
    void refreshMcp();
    const offSlack = onEvent("slack://status", setSlack);
    void refreshSlack();
    return () => {
      window.removeEventListener("focus", onFocus);
      offChange();
      offOpen();
      offState();
      offMcp();
      offSlack();
    };
  }, [refresh, refreshMcp, refreshSlack, navigate]);

  const openTask = useCallback(
    (task: string, focus: string | null = null) => {
      if (!route.task && !route.episode && document.activeElement instanceof HTMLElement) returnFocus.current = document.activeElement;
      navigate({ ...route, task, focus, episode: null });
    },
    [route, navigate],
  );

  const openEpisode = useCallback(
    (episode: string) => {
      if (!route.task && !route.episode && document.activeElement instanceof HTMLElement) returnFocus.current = document.activeElement;
      navigate({ ...route, task: null, focus: null, episode });
    },
    [route, navigate],
  );

  /** Closes whichever sheet is open and puts focus back where it was before. */
  const closeSheet = useCallback(() => {
    navigate({ ...route, task: null, focus: null, episode: null });
    const back = returnFocus.current;
    returnFocus.current = null;
    if (back?.isConnected) back.focus();
  }, [route, navigate]);

  const setView = useCallback(
    (view: View) => navigate({ view, task: route.task, focus: null, proposal: null, episode: null }),
    [route.task, navigate],
  );

  const decide = useCallback(
    async (p: Proposal, decision: ProposalDecision) => {
      if (!snap || deciding.has(p.id)) return;
      // Rust checks the task's current rev, not the one the proposal was raised against.
      const rev = snap.tasks.find((t) => t.id === p.taskId)?.rev ?? p.taskRev;
      setDeciding((s) => toggled(s, p.id, true));
      try {
        await decideProposal({ proposalId: p.id, decision, rev });
        setSnap((s) => s && { ...s, proposals: s.proposals.filter((x) => x.id !== p.id), lastProposalHandledAt: new Date().toISOString() });
        void emitChange({ taskId: p.taskId, proposalId: p.id });
        if (inTauri) await refresh();
      } catch (error) {
        setNotice(`Could not ${decision} the proposal: ${errorText(error)}`);
        void refresh();
      } finally {
        setDeciding((s) => toggled(s, p.id, false));
      }
    },
    [snap, deciding, refresh],
  );

  const ack = useCallback(
    async (taskId: string, change: Change) => {
      const task = snap?.tasks.find((t) => t.id === taskId);
      if (!task || acking.has(change.id)) return;
      setAcking((s) => toggled(s, change.id, true));
      try {
        await ackChange({ taskId, changeId: change.id, rev: task.rev });
        const at = new Date().toISOString();
        setSnap(
          (s) =>
            s && {
              ...s,
              tasks: s.tasks.map((t) =>
                t.id !== taskId
                  ? t
                  : {
                      ...t,
                      changes: t.changes.map((c) => (c.id === change.id && c.kind === "requirement" ? { ...c, ackedAt: at, ackedBy: "you" } : c)),
                    },
              ),
            },
        );
        void emitChange({ taskId, changeId: change.id });
        if (inTauri) await refresh();
      } catch (error) {
        setNotice(`Could not ack the change: ${errorText(error)}`);
        void refresh();
      } finally {
        setAcking((s) => toggled(s, change.id, false));
      }
    },
    [snap, acking, refresh],
  );

  const setFlag = useCallback(async (flag: "dnd" | "paused", on: boolean) => {
    try {
      setFlags(await (flag === "dnd" ? setDnd(on) : setPaused(on)));
    } catch (error) {
      setNotice(`Could not change the setting: ${errorText(error)}`);
    }
  }, []);

  // The proposal A / E / R act on: the top of the queue on Today, the routed one in Review.
  const current =
    route.view === "today"
      ? snap?.proposals[0]
      : route.view === "review"
        ? (snap?.proposals.find((p) => p.id === route.proposal) ?? snap?.proposals[0])
        : undefined;

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
        return;
      }
      if (e.key === "Escape") {
        if (route.task || route.episode) {
          e.preventDefault();
          closeSheet();
        } else if (notice) setNotice(null);
        return;
      }
      if (typing(e.target) || e.ctrlKey || e.metaKey || e.altKey || e.repeat || !current || deciding.has(current.id)) return;
      const k = e.key.toLowerCase();
      if (k === "a") void decide(current, "approve");
      else if (k === "r") void decide(current, "reject");
      else if (k === "e") openTask(current.taskId, "changes");
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [route.task, route.episode, current, deciding, notice, decide, openTask, closeSheet]);

  if (!snap) {
    return (
      <div className="win">
        <TitleBar ref={searchRef} query={query} onQuery={setQuery} />
        <div className="boot" role={loadError ? "alert" : "status"}>
          {loadError ? (
            <>
              <b>Relay could not read its database.</b>
              <span className="meta">{loadError}</span>
              <button type="button" className="btn" onClick={() => void refresh()}>
                Try again
              </button>
            </>
          ) : (
            <span className="meta">Loading…</span>
          )}
        </div>
      </div>
    );
  }

  const scope = todayScope(snap);
  const selected = route.task ? snap.tasks.find((t) => t.id === route.task) : undefined;
  const episode = selected ? null : route.episode;
  const top = snap.proposals[0];

  return (
    <div className="win">
      <TitleBar ref={searchRef} query={query} onQuery={setQuery} />
      <div className={`body${selected || episode ? " with-sheet" : ""}`}>
        <Rail
          view={route.view}
          onView={setView}
          version={snap.version}
          inbox={snap.inboxCount}
          review={snap.proposals.length}
          liveAgents={liveClaims(snap.tasks).length}
          sync={snap.sync}
          mcp={inTauri ? mcp : null}
          notice={<UpdateNotice update={update} />}
        />

        {route.view === "today" && (
          <section className="main">
            <TodayHeader snapshot={snap} />
            <div className="scroll">
              <TaskGroups snapshot={snap} query={query} selectedId={selected?.id ?? null} onSelect={openTask} />
              {top && !query && (
                <ProposalCard
                  size="full"
                  proposal={top}
                  task={snap.tasks.find((t) => t.id === top.taskId)}
                  position={{ index: 1, total: scope.needsReview }}
                  busy={deciding.has(top.id)}
                  current
                  onApprove={() => void decide(top, "approve")}
                  onEdit={() => openTask(top.taskId, "changes")}
                  onReject={() => void decide(top, "reject")}
                />
              )}
            </div>
            <AgentStrip tasks={snap.tasks} onSelect={openTask} />
          </section>
        )}
        {route.view === "inbox" && (
          <InboxView slack={slack} openId={episode} onOpen={openEpisode} onSettings={() => setView("settings")} />
        )}
        {route.view === "projects" && <ProjectsView snap={snap} selectedId={selected?.id ?? null} onSelect={openTask} />}
        {route.view === "agents" && <AgentsView snap={snap} onSelect={openTask} />}
        {route.view === "review" && (
          <ReviewView
            snap={snap}
            current={current}
            routed={route.proposal}
            busy={deciding}
            onDecide={(p, d) => void decide(p, d)}
            onEdit={(p) => openTask(p.taskId, "changes")}
            onOpenEpisode={openEpisode}
          />
        )}
        {route.view === "settings" && (
          <SettingsView
            snap={snap}
            update={update}
            flags={inTauri ? flags : null}
            onFlag={setFlag}
            mcp={inTauri ? mcp : null}
            onMcpRetry={() => void refreshMcp()}
            slack={slack}
            onSlack={setSlack}
          />
        )}

        {selected && (
          <TaskSheet
            key={selected.id}
            task={selected}
            project={snap.projects.find((p) => p.id === selected.projectId)}
            statuses={snap.statuses}
            focusChanges={route.focus === "changes"}
            onClose={closeSheet}
            onAck={(c) => void ack(selected.id, c)}
            onOpen={(url) => void openExternal(url)}
            acking={acking}
          />
        )}
        {episode && <EpisodeSheet key={episode} id={episode} onClose={closeSheet} />}
      </div>
      {notice && (
        <div className="toast" role="alert">
          <span>{notice}</span>
          <button type="button" className="ib" title="Dismiss (Esc)" onClick={() => setNotice(null)}>
            ×
          </button>
        </div>
      )}
    </div>
  );
}
