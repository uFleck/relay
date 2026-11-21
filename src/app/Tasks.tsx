import { useEffect, useMemo, useRef } from "react";
import { Icon } from "../components/Icon";
import type { V2Task, V2WorkspaceSnapshot } from "../ipc";
import { groupTasks, SIGIL, type TaskGroupId, type WorkspaceFilter } from "../workspace";

/* The v2 Tasks surface: three groups sorted by latest change, sharing the Notebook filter.
   The needs-you count and the sheet are owned by App and src/app/TaskSheet.tsx. */

interface Props {
  workspace: V2WorkspaceSnapshot | null;
  filter: WorkspaceFilter;
  selectedTaskId: string | null;
  onSelect: (id: string) => void;
}

const GROUPS: { id: TaskGroupId; label: string; hint: string }[] = [
  { id: "needsYou", label: "Needs you", hint: "Waiting on your decision or answer" },
  { id: "inFlight", label: "In flight", hint: "Agents at work" },
  { id: "closed", label: "Closed", hint: "Done and cancelled" },
];

export function Tasks({ workspace, filter, selectedTaskId, onSelect }: Props) {
  const headingRef = useRef<HTMLHeadingElement>(null);
  const rows = useRef(new Map<string, HTMLButtonElement>());
  const hadSelection = useRef(false);

  const groups = useMemo(
    () => groupTasks(workspace?.tasks ?? [], filter),
    [workspace, filter],
  );
  const notes = workspace?.notes ?? [];
  const projects = workspace?.projects ?? [];
  const total = groups.needsYou.length + groups.inFlight.length + groups.closed.length;

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

  const row = (task: V2Task) => {
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
  };

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
                ? `${total} matching ${total === 1 ? "task" : "tasks"}`
                : "Filtering…"
              : `${total} ${total === 1 ? "task" : "tasks"}`}
          </div>
        </div>
      </header>

      <div className="scroll ts-scroll">
        {workspace && total === 0 && (
          <p className="empty">
            {filter.filtering && filter.ready
              ? "No task matches this filter."
              : "No tasks yet. Add !, ? or ?? to a Note line to create one."}
          </p>
        )}
        {GROUPS.map(({ id, label, hint }) => {
          const tasks = groups[id];
          const heading = (
            <h2 className="ts-group-h">
              {label} <span className="meta">{tasks.length}</span>
              <span className="ts-group-hint meta">{hint}</span>
            </h2>
          );
          if (id === "closed") {
            return (
              <details className="ts-group closed" key={id}>
                <summary className="ts-group-summary">{heading}</summary>
                <div className="ts-group-rows">{tasks.map(row)}</div>
              </details>
            );
          }
          return (
            <section className="ts-group" key={id} aria-label={label}>
              {heading}
              <div className="ts-group-rows">{tasks.map(row)}</div>
            </section>
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
