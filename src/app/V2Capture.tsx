import { useCallback, useEffect, useState } from "react";
import { Composer } from "../components/Composer";
import {
  errorText,
  onEvent,
  v2NotebookSnapshot,
  v2TaskAssignProject,
  type V2NotebookSnapshot,
  type V2Project,
  type V2Task,
} from "../ipc";

/* A temporary v2 pane on the Notes route. It shares the Composer with the Dock and shows
   the saved Notes with their Task rows. The full Notebook timeline belongs to #26. */

interface Props {
  onNotice: (message: string) => void;
}

const SIGIL: Record<V2Task["agentMode"], string> = { act: "!", wait: "?", grill: "??" };

function TaskRow({
  task,
  projects,
  onAssign,
}: {
  task: V2Task;
  projects: V2Project[];
  onAssign: (task: V2Task, projectId: string) => void;
}) {
  const project = projects.find((candidate) => candidate.id === task.projectId);
  return (
    <div className={`v2-task mode-${task.agentMode}`} data-task-id={task.id}>
      <span className="v2-sigil">{SIGIL[task.agentMode]}</span>
      <span className="v2-task-line">{task.originalLine}</span>
      <span className="v2-task-status">{task.status.replace(/_/g, " ")}</span>
      {project ? (
        <span className="v2-project" title={project.ownerRepo ?? undefined}>
          {project.name}
        </span>
      ) : projects.length > 0 ? (
        <select
          className="v2-project-pick"
          aria-label={`Project for ${task.originalLine}`}
          value=""
          onChange={(event) => onAssign(task, event.target.value)}
        >
          <option value="">Choose project…</option>
          {projects.map((candidate) => (
            <option key={candidate.id} value={candidate.id}>
              {candidate.name}
            </option>
          ))}
        </select>
      ) : (
        <span className="meta">Register a project to assign this Task.</span>
      )}
      <span className="v2-refs">
        {task.references.map((reference) => (
          <span className="v2-ref" key={reference.id} title={reference.canonicalUrl}>
            {reference.targetId}
          </span>
        ))}
      </span>
    </div>
  );
}

export function V2Capture({ onNotice }: Props) {
  const [snapshot, setSnapshot] = useState<V2NotebookSnapshot | null>(null);
  const [editing, setEditing] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setSnapshot(await v2NotebookSnapshot());
    } catch (error) {
      onNotice(`Could not read the v2 notebook: ${errorText(error)}`);
    }
  }, [onNotice]);

  useEffect(() => {
    void reload();
    const off = onEvent("relay://v2-change", () => void reload());
    return off;
  }, [reload]);

  const assign = async (task: V2Task, projectId: string) => {
    try {
      await v2TaskAssignProject(task.id, projectId);
      await reload();
    } catch (error) {
      onNotice(`Could not assign the project: ${errorText(error)}`);
    }
  };

  const lines = (note: V2NotebookSnapshot["notes"][number]) => (
    <>
      {note.lines.map((line) => (
        <div className="v2-line" key={line.id}>
          <span className="v2-line-text">{line.text}</span>
          <span className="v2-refs">
            {line.references.map((reference) => (
              <span className="v2-ref" key={reference.id} title={reference.canonicalUrl}>
                {reference.targetId}
              </span>
            ))}
          </span>
          {note.tasks
            .filter((task) => task.sourceLineId === line.id)
            .map((task) => (
              <TaskRow key={task.id} task={task} projects={snapshot?.projects ?? []} onAssign={(t, id) => void assign(t, id)} />
            ))}
        </div>
      ))}
      {note.tasks
        .filter((task) => !note.lines.some((line) => line.id === task.sourceLineId))
        .map((task) => (
          <TaskRow key={task.id} task={task} projects={snapshot?.projects ?? []} onAssign={(t, id) => void assign(t, id)} />
        ))}
    </>
  );

  return (
    <section className="v2-capture" aria-label="v2 capture">
      <div className="v2-capture-head">
        <b>Capture</b>
        <span className="meta">Shared Composer · v2 notebook</span>
      </div>
      <Composer onSaved={() => void reload()} onNotice={onNotice} />
      <div className="v2-notes">
        {snapshot?.notes.length === 0 && (
          <p className="empty">No notes yet. A plain line becomes a Note; !, ? and ?? also create Tasks.</p>
        )}
        {snapshot?.notes.map((note) => (
          <article className="v2-note" key={note.id} data-note-id={note.id} data-revision={note.revision}>
            <div className="v2-note-head">
              <span className="meta">
                {new Date(note.createdAt).toLocaleString()} · rev {note.revision}
              </span>
              {editing === note.id ? (
                <button type="button" className="btn ghost" onClick={() => setEditing(null)}>
                  Close editor
                </button>
              ) : (
                <button type="button" className="btn ghost" onClick={() => setEditing(note.id)}>
                  Edit
                </button>
              )}
            </div>
            {editing === note.id ? (
              <Composer
                key={`${note.id}:${note.revision}`}
                note={note}
                onSaved={() => {
                  setEditing(null);
                  void reload();
                }}
                onNotice={onNotice}
                onReload={() => void reload()}
              />
            ) : (
              lines(note)
            )}
          </article>
        ))}
      </div>
    </section>
  );
}
