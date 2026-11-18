import { Fragment, useEffect, useRef } from "react";
import { clock } from "../model";
import type { Proposal, Task } from "../types";
import { Icon } from "./Icon";

/* One proposal component. `full` is the Today/Review card with Approve/Edit/Reject; `compact` is the
   hover card, whose only action is Review: the hover never changes tasks. */

const kindLabel: Record<Proposal["kind"], string> = {
  change_deadline: "due",
  add_requirement: "req",
  new_task: "new",
};

interface FullProps {
  size: "full";
  proposal: Proposal;
  task: Task | undefined;
  position: { index: number; total: number };
  /** A decision is in flight: the buttons wait for it. */
  busy?: boolean;
  /** The proposal the A/E/R keys act on. */
  current?: boolean;
  /** Scroll it into view, for a Review route that names it. */
  reveal?: boolean;
  onApprove: () => void;
  onEdit: () => void;
  onReject: () => void;
}

interface CompactProps {
  size: "compact";
  proposal: Proposal;
  task: Task | undefined;
  onReview: () => void;
}

export function ProposalCard(props: FullProps | CompactProps) {
  return props.size === "full" ? <FullCard {...props} /> : <CompactCard {...props} />;
}

function FullCard({ proposal: p, task, position, busy, current, reveal, onApprove, onEdit, onReject }: FullProps) {
  const ref = useRef<HTMLElement>(null);
  useEffect(() => {
    if (reveal) ref.current?.scrollIntoView({ block: "nearest" });
  }, [reveal]);

  return (
    <section ref={ref} className={`rq${current ? " cur" : ""}`} aria-label="Proposal" aria-busy={busy || undefined}>
      <div className="rq-h">
        <Icon name="review" small className="warn-fg" />
        <b>Proposal</b>
        <span className="meta ellip">
          {p.kind} · from {p.origin} with {p.author} · episode {p.episode}
        </span>
        <span className="meta push nowrap">
          {position.index} of {position.total} waiting
        </span>
      </div>
      <div className="rq-b">
        <div className="convo">
          {p.messages.map((m, i) => (
            <div key={i} className={`msg${m.me ? " me" : ""}`}>
              <span className="w">{clock(m.at)}</span>
              <div>
                <span className="who">{m.who}</span>
                {m.parts.map((part, j) => (part.match ? <mark key={j}>{part.text}</mark> : <span key={j}>{part.text}</span>))}
              </div>
            </div>
          ))}
        </div>
        <div className="jev">
          <div className="meta">
            JEV · {p.jev.stage} · {p.jev.ms} ms
          </div>
          {p.jev.scores.map((s) => (
            <div key={s.label} className={`bar tone-${s.tone}`}>
              <span>{s.label}</span>
              <span className="v">{s.value.toFixed(2)}</span>
              <span className="trk" title={`threshold ${s.threshold.toFixed(2)}`}>
                <i style={{ width: `${s.value * 100}%` }} />
                <u style={{ left: `${s.threshold * 100}%` }} />
              </span>
            </div>
          ))}
        </div>
      </div>
      <div className="diff">
        <div className="ctx">
          <span className="k">task</span>
          {task ? `${task.title} · rev ${p.taskRev}` : p.taskId}
        </div>
        {p.diff.map((d) => (
          <Fragment key={d.field}>
            {d.from && (
              <div className="del">
                <span className="k">− {d.field}</span>
                {d.from}
              </div>
            )}
            <div className="add">
              <span className="k">+ {d.field}</span>
              {d.to}
            </div>
          </Fragment>
        ))}
      </div>
      <div className="rq-f">
        <button type="button" className="btn pri" disabled={busy} onClick={onApprove}>
          <Icon name="check" small />
          Approve
        </button>
        <button type="button" className="btn" disabled={busy} onClick={onEdit}>
          <Icon name="pencil" small />
          Edit
        </button>
        <button type="button" className="btn bad" disabled={busy} onClick={onReject}>
          <Icon name="x" small />
          Reject
        </button>
        <span className="meta push">{busy ? "saving…" : `${p.note}${current ? " · A / E / R" : ""}`}</span>
      </div>
    </section>
  );
}

function CompactCard({ proposal: p, task, onReview }: CompactProps) {
  const affects = p.jev.scores[0];
  const d = p.diff[0];
  return (
    <div className="card prop">
      <div className="hd">
        <span className="meta kind">{kindLabel[p.kind]}</span>
        <b>{task?.title ?? p.taskId}</b>
        <span className="meta">
          {p.author.split(" ")[0]}
          {p.messages.length > 0 && ` · ${clock(p.messages[p.messages.length - 1].at)}`}
        </span>
      </div>
      {d && (
        <div className="dl">
          {d.from && (
            <>
              <s>{d.from}</s>→
            </>
          )}
          <span className="to">{d.to}</span>
        </div>
      )}
      {affects && (
        <div className="cf">
          <span>affects {affects.value.toFixed(2)}</span>
          <span className="trk">
            <i style={{ width: `${affects.value * 100}%` }} />
          </span>
        </div>
      )}
      <div className="acts">
        <button type="button" className="btn pri" onClick={onReview}>
          <Icon name="ext" />
          Review
        </button>
        <span className="meta">decide in the main window</span>
      </div>
    </div>
  );
}
