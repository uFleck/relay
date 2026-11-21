import { useEffect, useMemo, useRef, useState } from "react";
import { Composer } from "../components/Composer";
import type { V2Note, V2Project, V2Task, V2WorkspaceSnapshot } from "../ipc";
import {
  groupByDay,
  nearBottom,
  noteMatches,
  SIGIL,
  todayKey,
  type WorkspaceFilter,
} from "../workspace";

/* The v2 Notebook is one chronological timeline. Oldest entries sit at the top and the
   Composer sits beside the newest end, so the reader opens on today at the bottom. */

interface Props {
  workspace: V2WorkspaceSnapshot | null;
  filter: WorkspaceFilter;
  selectedTaskId: string | null;
  onOpenTask: (id: string) => void;
  onNotice: (message: string) => void;
  onSaved: () => void;
}

export function Notebook({ workspace, filter, selectedTaskId, onOpenTask, onNotice, onSaved }: Props) {
  const [editing, setEditing] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const followNewest = useRef(true);
  const seen = useRef<ReadonlySet<string>>(new Set());
  const editScroll = useRef<number | null>(null);
  const initialised = useRef(false);

  const notes = useMemo(
    () => (workspace?.notes ?? []).filter((note) => noteMatches(note, filter)),
    [workspace, filter],
  );
  const groups = useMemo(() => groupByDay(notes), [notes]);
  const projects = workspace?.projects ?? [];

  const scrollToToday = () => {
    const scroller = scrollRef.current;
    if (!scroller) return;
    const anchor = scroller.querySelector<HTMLElement>(`[data-day="${todayKey()}"]`);
    if (anchor) {
      scroller.scrollTop = anchor.offsetTop - scroller.offsetTop;
    } else {
      scroller.scrollTop = scroller.scrollHeight;
    }
    followNewest.current = true;
  };

  // Entering the view lands on today; a later committed write only moves a reader who is
  // already at the newest end. Someone reading older Notes keeps their position.
  useEffect(() => {
    if (!workspace || initialised.current) return;
    initialised.current = true;
    requestAnimationFrame(scrollToToday);
  }, [workspace]);

  useEffect(() => {
    if (!workspace) return;
    const ids = new Set(notes.map((note) => note.id));
    const added = [...ids].some((id) => !seen.current.has(id));
    seen.current = ids;
    if (added && followNewest.current && editing === null) {
      requestAnimationFrame(() => {
        const scroller = scrollRef.current;
        if (scroller) scroller.scrollTop = scroller.scrollHeight;
      });
    }
  }, [notes, workspace, editing]);

  const openEditor = (note: V2Note) => {
    editScroll.current = scrollRef.current?.scrollTop ?? null;
    setEditing(note.id);
  };

  const closeEditor = () => {
    setEditing(null);
    const at = editScroll.current;
    editScroll.current = null;
    if (at !== null) requestAnimationFrame(() => {
      if (scrollRef.current) scrollRef.current.scrollTop = at;
    });
  };

  return (
    <section className="main notebook" aria-label="Notebook">
      <header className="head nb-head">
        <div>
          <h1>Notebook</h1>
          <div className="date meta">
            {filter.filtering
              ? filter.ready
                ? `${notes.length} matching ${notes.length === 1 ? "note" : "notes"}`
                : "Filtering…"
              : `${notes.length} ${notes.length === 1 ? "note" : "notes"}`}
          </div>
        </div>
      </header>

      <div
        className="scroll nb-scroll"
        ref={scrollRef}
        onScroll={(event) => {
          followNewest.current = nearBottom(event.currentTarget);
        }}
      >
        {workspace && notes.length === 0 && (
          <p className="empty nb-empty">
            {filter.filtering && filter.ready
              ? "No note matches this filter."
              : "No notes yet. Capture the first one below."}
          </p>
        )}
        {groups.map((group) => (
          <section className="nb-day" key={group.key} data-day={group.key}>
            <h2 className="nb-day-h">
              <span>{group.label}</span>
              <span className="meta">{group.notes.length || "no notes"}</span>
            </h2>
            {group.notes.map((note) => (
              <NoteCard
                key={note.id}
                note={note}
                projects={projects}
                editing={editing === note.id}
                selectedTaskId={selectedTaskId}
                onEdit={() => openEditor(note)}
                onCloseEdit={closeEditor}
                onOpenTask={onOpenTask}
                onNotice={onNotice}
                onSaved={() => {
                  closeEditor();
                  onSaved();
                }}
              />
            ))}
          </section>
        ))}
        <div className="nb-composer">
          <div className="nb-composer-label meta">New note · newest at the bottom</div>
          <Composer onSaved={() => onSaved()} onNotice={onNotice} />
        </div>
      </div>
    </section>
  );
}

interface CardProps {
  note: V2Note;
  projects: V2Project[];
  editing: boolean;
  selectedTaskId: string | null;
  onEdit: () => void;
  onCloseEdit: () => void;
  onOpenTask: (id: string) => void;
  onNotice: (message: string) => void;
  onSaved: () => void;
}

function NoteCard({
  note,
  projects,
  editing,
  selectedTaskId,
  onEdit,
  onCloseEdit,
  onOpenTask,
  onNotice,
  onSaved,
}: CardProps) {
  const active = new Set(note.lines.map((line) => line.id));
  // A removed source line has no active row; its Task still belongs to this Note and is
  // drawn from the frozen original line rather than disappearing with the edit.
  const detached = note.tasks.filter((task) => !active.has(task.sourceLineId));

  return (
    <article className="nb-note" data-note-id={note.id} data-revision={note.revision}>
      <div className="nb-note-head">
        <span className="nb-dot" aria-hidden="true" />
        <span className="meta">{new Date(note.createdAt).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" })}</span>
        <span className="meta">{note.tasks.length ? `${note.tasks.length} task${note.tasks.length === 1 ? "" : "s"}` : "note"}</span>
        <span className="push" />
        {editing ? (
          <button type="button" className="btn ghost" onClick={onCloseEdit}>
            Close editor
          </button>
        ) : (
          <button type="button" className="btn ghost" onClick={onEdit}>
            Edit
          </button>
        )}
      </div>

      {editing ? (
        <Composer
          key={`${note.id}:${note.revision}`}
          note={note}
          onSaved={onSaved}
          onNotice={onNotice}
          onReload={onSaved}
        />
      ) : (
        <div className="nb-lines">
          {note.lines.map((line) => (
            <div className="nb-line" key={line.id}>
              <div className="nb-line-text">{line.text || <span className="meta">empty line</span>}</div>
              {line.references.length > 0 && (
                <div className="nb-refs">
                  {line.references.map((reference) => (
                    <span className="nb-ref" key={reference.id} title={reference.canonicalUrl}>
                      @{reference.targetId}
                    </span>
                  ))}
                </div>
              )}
              {note.tasks
                .filter((task) => task.sourceLineId === line.id)
                .map((task) => (
                  <TaskBranch
                    key={task.id}
                    task={task}
                    project={projects.find((candidate) => candidate.id === task.projectId)}
                    selected={task.id === selectedTaskId}
                    onOpen={() => onOpenTask(task.id)}
                  />
                ))}
            </div>
          ))}
          {detached.map((task) => (
            <TaskBranch
              key={task.id}
              task={task}
              project={projects.find((candidate) => candidate.id === task.projectId)}
              selected={task.id === selectedTaskId}
              onOpen={() => onOpenTask(task.id)}
              detached
            />
          ))}
        </div>
      )}
    </article>
  );
}

function TaskBranch({
  task,
  project,
  selected,
  onOpen,
  detached,
}: {
  task: V2Task;
  project: V2Project | undefined;
  selected: boolean;
  onOpen: () => void;
  detached?: boolean;
}) {
  return (
    <button
      type="button"
      className={`nb-branch mode-${task.agentMode}${selected ? " on" : ""}${detached ? " detached" : ""}`}
      data-task-id={task.id}
      onClick={onOpen}
      title={detached ? "Source line removed; showing the saved Task line" : "Open Task"}
    >
      <span className="nb-sigil">{SIGIL[task.agentMode]}</span>
      <span className="nb-branch-line">{task.originalLine}</span>
      <span className="nb-status">{task.status.replace(/_/g, " ")}</span>
      {project && <span className="nb-project">{project.name}</span>}
    </button>
  );
}
