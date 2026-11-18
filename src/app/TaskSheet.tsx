import { Fragment, useEffect, useRef } from "react";
import { Icon } from "../components/Icon";
import { linkIcon, ProjectChip, Prose } from "../components/bits";
import { claimState, clock, elapsed, isUnacked, statusColor, useNow } from "../model";
import type { Change, Project, Status, Task } from "../types";
import { TaskContext } from "./Github";

interface Props {
  task: Task;
  project: Project | undefined;
  statuses: Status[];
  focusChanges: boolean;
  onClose: () => void;
  onAck: (change: Change) => void;
  onOpen: (url: string) => void;
  /** Change ids whose ack is in flight. */
  acking: ReadonlySet<string>;
  /** Drawn as a card inside a list (Projects) instead of the column beside it. */
  inline?: boolean;
}

/** The task detail column, or a card under its row when `inline`. Not a route: `?task=` drives it and Esc closes it. */
export function TaskSheet({ task, project, statuses, focusChanges, onClose, onAck, onOpen, acking, inline }: Props) {
  const sheetRef = useRef<HTMLElement>(null);
  const changesRef = useRef<HTMLDivElement>(null);
  const unacked = task.changes.filter(isUnacked).length;
  const changes = [...task.changes].sort((a, b) => Date.parse(b.at) - Date.parse(a.at));
  const thread = task.links.find((l) => l.kind === "thread");
  const pr = task.links.find((l) => l.kind === "pr");

  useEffect(() => {
    if (focusChanges) changesRef.current?.scrollIntoView({ block: "start" });
  }, [focusChanges, task.id]);

  // Keyboard users land in the sheet; Esc (in App) closes it and returns focus to where it came from.
  useEffect(() => {
    sheetRef.current?.focus({ preventScroll: true });
    if (inline) sheetRef.current?.scrollIntoView({ block: "nearest" });
  }, [task.id, inline]);

  return (
    <aside ref={sheetRef} className={`sheet${inline ? " inline" : ""}`} tabIndex={-1} aria-label={`Task ${task.title}`}>
      <div className="sh-h">
        <div className="sh-top">
          <ProjectChip project={project} />
          <span className="meta">
            {task.ref} · rev {task.rev}
          </span>
          <span className="push" />
          <button type="button" className="ib" title="Close (Esc)" onClick={onClose}>
            <Icon name="x" small />
          </button>
        </div>
        <h2>{task.title}</h2>
        <Stepper statuses={statuses} current={task.statusId} />
        <ClaimBanner task={task} />
      </div>

      <div className="sh-b">
        <section className="sec">
          <h3>Description</h3>
          <div className="desc">
            <Prose text={task.description} />
          </div>
        </section>

        <TaskContext key={task.projectId} task={task} onOpen={onOpen} />

        <section className="sec" ref={changesRef}>
          <h3>
            Changes {unacked > 0 && <span className="bdg warn">{unacked}</span>}
          </h3>
          {changes.length ? (
            <ChangeTimeline changes={changes} onAck={onAck} onOpen={onOpen} acking={acking} />
          ) : (
            <p className="desc">No changes since the task was created.</p>
          )}
        </section>

        {task.links.length > 0 && (
          <section className="sec">
            <h3>Links</h3>
            <div className="links">
              {task.links.map((l) => (
                <button type="button" className="lnk" key={l.url} onClick={() => onOpen(l.url)} title={l.url}>
                  <Icon name={linkIcon[l.kind]} small className={l.kind === "pr" ? `pr-${l.state}` : undefined} />
                  <span className={l.kind === "branch" ? "mono" : undefined}>{l.label}</span>
                  <span className="meta">{l.state}</span>
                </button>
              ))}
            </div>
          </section>
        )}
      </div>

      <div className="sh-f">
        <button type="button" className="btn pri" disabled={!thread} onClick={() => thread && onOpen(thread.url)}>
          <Icon name="ext" small />
          Open thread
        </button>
        <button type="button" className="btn" disabled={!pr} onClick={() => pr && onOpen(`${pr.url}/files`)}>
          <Icon name="diff" small />
          View diff
        </button>
      </div>
    </aside>
  );
}

/** The default workflow without Backlog; Blocked sits off the line. */
function Stepper({ statuses, current }: { statuses: Status[]; current: string }) {
  const steps = statuses.filter((s) => s.id !== "backlog" && s.category !== "blocked");
  const cur = statuses.find((s) => s.id === current);
  const at = steps.findIndex((s) => s.id === current);
  const color = cur ? statusColor(cur.category) : "var(--text-tertiary)";

  return (
    <div style={{ "--step": color } as React.CSSProperties}>
      <div className="step">
        {steps.map((s, i) => (
          <Fragment key={s.id}>
            {i > 0 && <i className={`ln${i <= at ? " done" : ""}`} />}
            <span className={`dt${i < at ? " done" : i === at ? " cur" : ""}`} title={s.name} />
          </Fragment>
        ))}
      </div>
      <div className="steplbl">
        {steps.map((s, i) => (i === at ? <b key={s.id}>{s.short}</b> : <span key={s.id}>{s.short}</span>))}
      </div>
      {at < 0 && cur && <div className="meta off-line">{cur.name} · off the workflow until unblocked</div>}
    </div>
  );
}

function ClaimBanner({ task }: { task: Task }) {
  const now = useNow();
  const c = task.claim;
  if (!c) return <div className="banner idle">Unclaimed · any agent can pick this up</div>;
  const state = claimState(c, now);
  return (
    <>
      <div className={`banner ${state}`}>
        <span className="live" />
        <span>
          {state === "released" ? (
            <>
              <b>Released by {c.holder}</b> · {elapsed(c.releasedAt!, now)} ago
            </>
          ) : (
            <>
              <b>Claimed in {c.holder}</b> · {elapsed(c.claimedAt, now)}{" "}
              <span className="sub">· last progress {elapsed(c.lastProgressAt, now)} ago</span>
            </>
          )}
        </span>
        <span className="meta push">{c.id}</span>
      </div>
      <div className="meta progress">“{c.lastProgress}”</div>
    </>
  );
}

const sourceIcon = { slack: "slack", github: "github", clickup: "clickup", agent: "robot" } as const;

function ChangeTimeline({
  changes,
  onAck,
  onOpen,
  acking,
}: {
  changes: Change[];
  onAck: (c: Change) => void;
  onOpen: (url: string) => void;
  acking: ReadonlySet<string>;
}) {
  return (
    <div className="tl">
      {changes.map((c) =>
        c.kind === "field" ? (
          <div className="ev field" key={c.id}>
            <span className="n" />
            <div className="hd">
              <span className="meta">{clock(c.at)}</span>
              <Icon name={sourceIcon[c.source]} small className="muted-fg" />
              <span className="fs">
                {c.field}{" "}
                <b>
                  {c.from} → {c.to}
                </b>
              </span>
              <span className="meta push">{c.ref}</span>
            </div>
          </div>
        ) : (
          <div className={`ev ${c.ackedAt ? "ack" : "unack"}`} key={c.id}>
            <span className="n" />
            <div>
              <div className="hd">
                <span className="meta">{clock(c.at)}</span>
                <Icon name={sourceIcon[c.source]} small className="muted-fg" />
                <b className="fs">{c.author}</b>
                <span className="tag mut">requirement</span>
              </div>
              <q>“{c.text}”</q>
              <div className="hd">
                {c.ackedAt ? (
                  <>
                    <span className="tag ok">acked {clock(c.ackedAt)}</span>
                    <span className="meta">
                      {c.confidence.toFixed(2)} · by {c.ackedBy}
                    </span>
                  </>
                ) : (
                  <>
                    <span className="tag warn">unacked</span>
                    <span className="meta">confidence {c.confidence.toFixed(2)}</span>
                  </>
                )}
                <span className="act push">
                  {c.permalink && (
                    <button type="button" className="linkish" onClick={() => onOpen(c.permalink)}>
                      permalink
                    </button>
                  )}
                  {!c.ackedAt && (
                    <button type="button" className="linkish" disabled={acking.has(c.id)} onClick={() => onAck(c)}>
                      {acking.has(c.id) ? "acking…" : "ack"}
                    </button>
                  )}
                </span>
              </div>
            </div>
          </div>
        ),
      )}
    </div>
  );
}
