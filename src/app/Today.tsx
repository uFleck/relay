import { memo } from "react";
import { Icon } from "../components/Icon";
import { linkIcon, ProjectChip } from "../components/bits";
import { claimState, dueLabel, elapsed, headerDate, isUnacked, plural, statusColor, todayScope, useNow } from "../model";
import type { Project, Snapshot, Status, Task } from "../types";

const catStyle = (s: Status) => ({ "--cat": statusColor(s.category) }) as React.CSSProperties;

export function TodayHeader({ snapshot }: { snapshot: Snapshot }) {
  const now = useNow();
  const scope = todayScope(snapshot);
  const chip = (cat: string, n: number, label: string) => (
    <span className="chipc">
      <span className="dot" style={{ "--c": `var(--c-${cat})` } as React.CSSProperties} />
      <b>{n}</b>
      {label}
    </span>
  );
  return (
    <header className="head">
      <div>
        <h1>Today</h1>
        <div className="date meta nowrap">
          {headerDate(now)} · {plural(scope.total, "task")} · {plural(scope.projects, "project")}
        </div>
      </div>
      <div className="sum">
        {chip("active", scope.inProgress, "in progress")}
        {chip("blocked", scope.blocked, "blocked")}
        {chip("review", scope.needsReview, "needs review")}
      </div>
    </header>
  );
}

interface GroupsProps {
  snapshot: Snapshot;
  query: string;
  selectedId: string | null;
  onSelect: (id: string) => void;
  /** Shown when nothing is open and there is no query. */
  empty?: string;
}

/** Groups follow the workflow order; empty groups are hidden; rows carry no status pill. */
export function TaskGroups({ snapshot, query, selectedId, onSelect, empty }: GroupsProps) {
  const q = query.trim().toLowerCase();
  const { tasks } = todayScope(snapshot);
  const visible = q
    ? tasks.filter((t) => t.title.toLowerCase().includes(q) || t.ref.toLowerCase().includes(q))
    : tasks;
  const projects = new Map(snapshot.projects.map((p) => [p.id, p]));

  const groups = snapshot.statuses
    .map((status) => ({ status, rows: visible.filter((t) => t.statusId === status.id) }))
    .filter((g) => g.rows.length);

  if (!groups.length) {
    if (q) return <p className="empty">No task on Today matches “{query}”.</p>;
    return (
      <p className="empty">
        {empty ??
          (snapshot.tasks.length
            ? "Nothing due or in flight today."
            : "No tasks yet. Tasks appear here when an agent or an approved proposal creates one.")}
      </p>
    );
  }

  return (
    <>
      {groups.map(({ status, rows }) => (
        <div key={status.id} role="group" aria-label={status.name}>
          <div className="grp" style={catStyle(status)}>
            <span className="dot" />
            {status.name} <span className="n">{rows.length}</span>
          </div>
          {rows.map((t) => (
            <TaskRow
              key={t.id}
              task={t}
              status={status}
              project={projects.get(t.projectId)}
              selected={t.id === selectedId}
              onSelect={onSelect}
            />
          ))}
        </div>
      ))}
    </>
  );
}

interface RowProps {
  task: Task;
  status: Status;
  project: Project | undefined;
  selected: boolean;
  onSelect: (id: string) => void;
}

const TaskRow = memo(function TaskRow({ task, status, project, selected, onSelect }: RowProps) {
  const now = useNow();
  const unacked = task.changes.filter(isUnacked).length;
  const due = task.due && dueLabel(task.due, now);
  const kinds = [...new Set(task.links.map((l) => l.kind))].slice(0, 3);

  return (
    <button
      type="button"
      className={`row${selected ? " sel" : ""}`}
      style={catStyle(status)}
      onClick={() => onSelect(task.id)}
      aria-pressed={selected}
    >
      <span className="chk" title={status.name} />
      <span className="t" title={task.title}>
        {task.title}
        {unacked > 0 && <span className="new" title={`${unacked} unacked change${unacked > 1 ? "s" : ""}`} />}
      </span>
      <span>
        <ProjectChip project={project} />
      </span>
      <ClaimCell task={task} now={now} />
      <span className={`due ${due?.tone ?? ""}`}>{due?.text ?? "—"}</span>
      <span className="lk">
        {kinds.map((k) => (
          <Icon key={k} name={linkIcon[k]} small />
        ))}
      </span>
    </button>
  );
});

function ClaimCell({ task, now }: { task: Task; now: number }) {
  const c = task.claim;
  if (!c) return <span className="claim none">unclaimed</span>;
  const state = claimState(c, now);
  if (state === "released") return <span className="claim none">released {elapsed(c.releasedAt!, now)}</span>;
  return (
    <span className="claim" title={`last progress ${elapsed(c.lastProgressAt, now)} ago`}>
      <span className={`live ${state}`} />
      {c.holder} · {elapsed(c.claimedAt, now)}
    </span>
  );
}

export function AgentStrip({ tasks, onSelect }: { tasks: Task[]; onSelect: (id: string) => void }) {
  const now = useNow();
  const live = tasks
    .filter((t) => t.claim && !t.claim.releasedAt)
    .sort((a, b) => Date.parse(b.claim!.claimedAt) - Date.parse(a.claim!.claimedAt));
  return (
    <footer className="strip">
      <span className="lbl">Agents</span>
      {live.length === 0 && <span className="meta">No live claims</span>}
      {live.map((t) => {
        const c = t.claim!;
        return (
          <button type="button" key={c.id} className="ag" onClick={() => onSelect(t.id)} title={c.lastProgress}>
            <span className={`live ${claimState(c, now)}`} />
            <b>
              {c.holder} · {elapsed(c.claimedAt, now)}
            </b>
            <span className="p">{t.title}</span>
            <span className="meta">· {elapsed(c.lastProgressAt, now)} ago</span>
          </button>
        );
      })}
    </footer>
  );
}
