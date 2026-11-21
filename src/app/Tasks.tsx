import { useEffect, useMemo, useRef } from "react";
import { Icon } from "../components/Icon";
import type { V2Project, V2Task, V2WorkspaceSnapshot } from "../ipc";
import { SIGIL, taskMatches, type WorkspaceFilter } from "../workspace";

/* The v2 Tasks surface. #26 supplies a flat filtered list and a minimal read-only sheet;
   #27 owns grouping, the Needs-you badge and the full sheet. The selected Task is resolved
   from the unfiltered workspace by App, so a filter cannot hide an open sheet. */

interface Props {
  workspace: V2WorkspaceSnapshot | null;
  filter: WorkspaceFilter;
  selectedTaskId: string | null;
  onSelect: (id: string) => void;
}

export function Tasks({
  workspace,
  filter,
  selectedTaskId,
  onSelect,
}: Props) {
  const headingRef = useRef<HTMLHeadingElement>(null);
  const rows = useRef(new Map<string, HTMLButtonElement>());
  const hadSelection = useRef(false);

  const tasks = useMemo(() => {
    const all = workspace?.tasks ?? [];
    return all
      .filter((task) => taskMatches(task, filter))
      .sort((a, b) => Date.parse(b.updatedAt) - Date.parse(a.updatedAt) || a.id.localeCompare(b.id));
  }, [workspace, filter]);

  const notes = workspace?.notes ?? [];
  const projects = workspace?.projects ?? [];

  // Esc routes back to /tasks; focus returns to the row that opened the sheet, or to the
  // heading when that row is no longer available.
  useEffect(() => {
    if (selectedTaskId) {
      hadSelection.current = true;
      return;
    }
    if (!hadSelection.current) return;
    hadSelection.current = false;
    requestAnimationFrame(() => {
      const row = selectedTaskId ? rows.current.get(selectedTaskId) : undefined;
      const first = rows.current.values().next().value as HTMLButtonElement | undefined;
      (row ?? first ?? headingRef.current)?.focus();
    });
  }, [selectedTaskId]);

  return (
    <section className="main tasks" aria-label="Tasks">
      <header className="head">
        <div>
          <h1 ref={headingRef} tabIndex={-1}>
            Tasks
          </h1>
          <div className="date meta">
            {filter.filtering
              ? filter.ready
                ? `${tasks.length} matching ${tasks.length === 1 ? "task" : "tasks"}`
                : "Filtering…"
              : `${tasks.length} ${tasks.length === 1 ? "task" : "tasks"}`}
          </div>
        </div>
      </header>

      <div className="scroll ts-scroll">
        {workspace && tasks.length === 0 && (
          <p className="empty">
            {filter.filtering && filter.ready
              ? "No task matches this filter."
              : "No tasks yet. Add !, ? or ?? to a Note line to create one."}
          </p>
        )}
        {tasks.map((task) => {
          const note = notes.find((candidate) => candidate.id === task.noteId);
          const project = projects.find((candidate) => candidate.id === task.projectId);
          return (
            <button
              type="button"
              key={task.id}
              ref={(element) => {
                if (element) rows.current.set(task.id, element);
                else rows.current.delete(task.id);
              }}
              className={`ts-row${task.id === selectedTaskId ? " on" : ""}`}
              data-task-row={task.id}
              onClick={() => onSelect(task.id)}
            >
              <span className={`ts-sigil mode-${task.agentMode}`}>{SIGIL[task.agentMode]}</span>
              <span className="ts-main">
                <b>{task.originalLine}</b>
                <span className="meta">
                  {note ? note.lines.find((line) => line.id === task.sourceLineId)?.text ?? task.originalLine : task.noteId}
                </span>
              </span>
              <span className="ts-status">{task.status.replace(/_/g, " ")}</span>
              {project && <span className="ts-project">{project.name}</span>}
            </button>
          );
        })}
      </div>
    </section>
  );
}

/** Rendered as the third column of `.body`, so a sheet never collapses the Tasks list. */
export function MissingTask({ id, onClose }: { id: string; onClose: () => void }) {
  return (
    <aside className="sheet missing" tabIndex={-1} aria-label="Task not found">
      <div className="sh-h">
        <b>Task not found</b>
        <button type="button" className="ib" title="Close" onClick={onClose}>
          <Icon name="x" small />
        </button>
      </div>
      <div className="sh-b">
        <p className="desc">
          No Task with id <span className="mono">{id}</span> exists in this workspace.
        </p>
      </div>
    </aside>
  );
}

export function TaskDetail({
  task,
  project,
  onClose,
}: {
  task: V2Task;
  project: V2Project | undefined;
  onClose: () => void;
}) {
  return (
    <aside className="sheet v2-sheet" tabIndex={-1} aria-label={`Task ${task.originalLine}`}>
      <div className="sh-h">
        <div className="sh-top">
          <span className={`tag mode-${task.agentMode}`}>{SIGIL[task.agentMode]} {task.agentMode}</span>
          <span className="meta mono">{task.id}</span>
          <span className="push" />
          <button type="button" className="ib" title="Close (Esc)" onClick={onClose}>
            <Icon name="x" small />
          </button>
        </div>
        <h2>{task.originalLine}</h2>
        <div className="meta">{task.status.replace(/_/g, " ")}</div>
      </div>
      <div className="sh-b">
        {project && (
          <section className="sec">
            <h3>Project</h3>
            <p className="desc">{project.name}</p>
          </section>
        )}
        <section className="sec">
          <h3>References</h3>
          {task.references.length === 0 ? (
            <p className="desc">None.</p>
          ) : (
            <div className="nb-refs">
              {task.references.map((reference) => (
                <span className="nb-ref" key={reference.id} title={reference.canonicalUrl}>
                  @{reference.targetId}
                </span>
              ))}
            </div>
          )}
        </section>
        <section className="sec">
          <h3>Brief</h3>
          <p className="desc">{task.brief ?? "No Brief yet."}</p>
        </section>
        <section className="sec">
          <h3>Times</h3>
          <p className="desc meta">
            Created {task.createdAt} · updated {task.updatedAt}
          </p>
        </section>
      </div>
    </aside>
  );
}
