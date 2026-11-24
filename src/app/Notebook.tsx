import { useEffect, useMemo, useRef, useState } from "react";
import { Composer } from "../components/Composer";
import type { DraftSummary } from "../directives";
import type { V2Note, V2Project, V2Reference, V2Task, V2WorkspaceSnapshot } from "../ipc";
import { useEntering } from "../ui/motion";
import {
  groupByDay,
  nearBottom,
  noteMatches,
  SIGIL,
  statusTone,
  todayKey,
  type WorkspaceFilter,
} from "../workspace";

/* The v2 Notebook is one chronological timeline. Oldest entries sit at the top and the
   Composer sits beside the newest end, so the reader opens on today at the bottom. */

const dayDate = (key: string) =>
  new Date(`${key}T12:00:00`).toLocaleDateString("en-GB", {
    weekday: "short",
    day: "numeric",
    month: "short",
  });

interface Props {
  workspace: V2WorkspaceSnapshot | null;
  filter: WorkspaceFilter;
  /** A Note opened from search: scroll to it and show its detail editor. */
  focusNoteId?: string | null;
  selectedTaskId: string | null;
  onOpenTask: (id: string) => void;
  onNotice: (message: string) => void;
  onSaved: () => void;
  onDraftChange?: (draft: DraftSummary) => void;
}

export function Notebook({ workspace, filter, focusNoteId, selectedTaskId, onOpenTask, onNotice, onSaved, onDraftChange }: Props) {
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
  const enteringNotes = useEntering(notes.map((note) => note.id));
  const enteringTasks = useEntering(notes.flatMap((note) => note.tasks.map((task) => task.id)));

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

  // A search result addresses a Note by id. Bring it into view and open its editor once.
  useEffect(() => {
    if (!focusNoteId || !workspace) return;
    const note = workspace.notes.find((candidate) => candidate.id === focusNoteId);
    if (!note) return;
    requestAnimationFrame(() => {
      scrollRef.current
        ?.querySelector<HTMLElement>(`[data-note-id="${focusNoteId}"]`)
        ?.scrollIntoView({ block: "center" });
      editScroll.current = scrollRef.current?.scrollTop ?? null;
      setEditing(focusNoteId);
    });
  }, [focusNoteId, workspace]);

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
      <div
        className="scroll nb-scroll"
        data-parity="timeline"
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
              <span>{group.label}</span>{" "}
              <span className="meta">{dayDate(group.key)}</span>
            </h2>
            {group.notes.map((note) => (
              <NoteCard
                key={note.id}
                note={note}
                projects={projects}
                editing={editing === note.id}
                selectedTaskId={selectedTaskId}
                fresh={enteringNotes.has(note.id)}
                freshTasks={enteringTasks}
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
      </div>
      <div className="nb-dock" data-parity="composer-dock">
        <Composer onSaved={() => onSaved()} onNotice={onNotice} onDraftChange={onDraftChange} />
      </div>
    </section>
  );
}

interface CardProps {
  note: V2Note;
  projects: V2Project[];
  editing: boolean;
  selectedTaskId: string | null;
  fresh: boolean;
  freshTasks: ReadonlySet<string>;
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
  fresh,
  freshTasks,
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
    <article
      className={`nb-note${fresh ? " fresh" : ""}`}
      data-note-id={note.id}
      data-revision={note.revision}
      data-mode={note.tasks[0]?.agentMode ?? undefined}
    >
      <span className="nb-dot" aria-hidden="true" />
      <div className="nb-note-head">
        <span className="meta">
          {new Date(note.createdAt).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" })}
          {note.source === "import" && note.lines[0]?.references[0]
            ? ` · imported ${note.lines[0].references[0].kind === "github" ? "PR" : "Card"}`
            : ""}
        </span>
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
          {note.lines.map((line) => {
            const lineTasks = note.tasks.filter((task) => task.sourceLineId === line.id);
            return (
              <div className="nb-line" key={line.id}>
                <div className="nb-line-text"><InlineLine text={line.text} references={line.references} /></div>
                {lineTasks.length > 0 && (
                  <div className="nb-branches">
                    {lineTasks.map((task) => (
                      <TaskBranch
                        key={task.id}
                        task={task}
                        project={projects.find((candidate) => candidate.id === task.projectId)}
                        selected={task.id === selectedTaskId}
                        fresh={freshTasks.has(task.id)}
                        onOpen={() => onOpenTask(task.id)}
                      />
                    ))}
                  </div>
                )}
              </div>
            );
          })}
          {detached.length > 0 && (
            <div className="nb-branches">
              {detached.map((task) => (
                <TaskBranch
                  key={task.id}
                  task={task}
                  project={projects.find((candidate) => candidate.id === task.projectId)}
                  selected={task.id === selectedTaskId}
                  fresh={freshTasks.has(task.id)}
                  onOpen={() => onOpenTask(task.id)}
                  detached
                />
              ))}
            </div>
          )}
        </div>
      )}
    </article>
  );
}

function InlineLine({ text, references }: { text: string; references: V2Reference[] }) {
  if (references.length === 0) return <>{text || <span className="meta">empty line</span>}</>;
  const matches = references
    .map((reference) => ({ reference, at: text.indexOf(`@${reference.targetId}`) }))
    .filter((match) => match.at >= 0)
    .sort((a, b) => a.at - b.at);
  const out: React.ReactNode[] = [];
  let cursor = 0;
  for (const { reference, at } of matches) {
    if (at < cursor) continue;
    out.push(text.slice(cursor, at));
    out.push(
      <span className="nb-ref" key={reference.id} title={reference.canonicalUrl}>
        @{reference.targetId}
      </span>,
    );
    cursor = at + reference.targetId.length + 1;
  }
  out.push(text.slice(cursor));
  return <>{out}</>;
}

function TaskBranch({
  task,
  project,
  selected,
  fresh,
  onOpen,
  detached,
}: {
  task: V2Task;
  project: V2Project | undefined;
  selected: boolean;
  fresh: boolean;
  onOpen: () => void;
  detached?: boolean;
}) {
  return (
    <div
      role="button"
      tabIndex={0}
      className={`nb-branch mode-${task.agentMode}${selected ? " on" : ""}${detached ? " detached" : ""}${fresh ? " fresh" : ""}`}
      data-task-id={task.id}
      data-tone={statusTone(task.status)}
      onClick={onOpen}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onOpen();
        }
      }}
      title={detached ? "Source line removed; showing the saved Task line" : "Open Task"}
    >
      <span className="nb-sigil">{SIGIL[task.agentMode]}</span>
      {task.originalLine}
      <span className="nb-status">{task.status.replace(/_/g, " ")}</span>
      {project && <span className="nb-project">+{project.name}</span>}
    </div>
  );
}
