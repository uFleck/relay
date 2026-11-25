import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { Composer } from "../components/Composer";
import { DeleteConfirmation } from "../components/DeleteConfirmation";
import { EMPTY_DRAFT, lineDirective, type DraftSummary } from "../directives";
import { errorText, v2NoteDelete, v2WorkspaceSnapshot, type V2Note, type V2Project, type V2Reference, type V2Task, type V2WorkspaceSnapshot } from "../ipc";
import { useEntering } from "../ui/motion";
import { PageBar } from "./PageBar";
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
  onCloseNote: () => void;
  selectedTaskId: string | null;
  onOpenTask: (id: string) => void;
  onNotice: (message: string) => void;
  onSaved: () => void;
  onDraftChange?: (draft: DraftSummary) => void;
}

type ComposerMode = { kind: "closed" | "capture" } | { kind: "edit"; note: V2Note; returnToCapture: boolean };

export function Notebook({ workspace, filter, focusNoteId, onCloseNote, selectedTaskId, onOpenTask, onNotice, onSaved, onDraftChange }: Props) {
  const [mode, setMode] = useState<ComposerMode>({ kind: "closed" });
  const editing = mode.kind === "edit" ? mode.note.id : null;
  const newNoteButton = useRef<HTMLButtonElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const followNewest = useRef(true);
  const seen = useRef<ReadonlySet<string>>(new Set());
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
    setMode((current) => current.kind === "edit" ? current : {
      kind: "edit",
      note,
      returnToCapture: current.kind === "capture",
    });
  };

  const openedFromSearch = useRef<string | null>(null);
  useEffect(() => {
    if (!focusNoteId) {
      openedFromSearch.current = null;
      return;
    }
    if (editing || openedFromSearch.current === focusNoteId) return;
    const note = workspace?.notes.find((candidate) => candidate.id === focusNoteId);
    if (!note) return;
    openedFromSearch.current = focusNoteId;
    openEditor(note);
    requestAnimationFrame(() => scrollRef.current
      ?.querySelector<HTMLElement>(`[data-note-id="${focusNoteId}"]`)
      ?.scrollIntoView({ block: "center" }));
  }, [editing, focusNoteId, workspace]);

  const closeComposer = () => {
    const resumeDraft = mode.kind === "edit" && mode.returnToCapture;
    setMode({ kind: resumeDraft ? "capture" : "closed" });
    if (editing && focusNoteId === editing) onCloseNote();
    if (!resumeDraft) {
      onDraftChange?.(EMPTY_DRAFT);
      requestAnimationFrame(() => {
        const editButton = editing
          ? scrollRef.current?.querySelector<HTMLButtonElement>(`[data-note-id="${editing}"] .nb-edit`)
          : null;
        (editButton ?? newNoteButton.current)?.focus({ preventScroll: true });
      });
    }
  };

  const reloadNote = async (id: string) => {
    try {
      const latest = (await v2WorkspaceSnapshot()).notes.find((note) => note.id === id);
      if (!latest) {
        onNotice("This note has been deleted.");
        closeComposer();
        return;
      }
      setMode((current) => current.kind === "edit" && current.note.id === id
        ? { ...current, note: latest }
        : current);
    } catch (cause) {
      onNotice(`Could not reload the note: ${errorText(cause)}`);
    }
  };

  return (
    <section className="main notebook" aria-label="Notebook">
      <PageBar
        title="Notebook"
        meta={`${notes.length} ${notes.length === 1 ? "note" : "notes"}`}
        actions={
          <button
            ref={newNoteButton}
            type="button"
            className="btn"
            onClick={() => setMode({ kind: "capture" })}
            disabled={mode.kind !== "closed"}
          >
            New note
          </button>
        }
      />
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
              : "No notes yet."}
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
                editorOpen={editing !== null}
                selectedTaskId={selectedTaskId}
                fresh={enteringNotes.has(note.id)}
                freshTasks={enteringTasks}
                onEdit={() => openEditor(note)}
                onOpenTask={onOpenTask}
                onSaved={onSaved}
              />
            ))}
          </section>
        ))}
      </div>
      <div className="nb-dock" hidden={mode.kind === "closed"} data-parity="composer-dock">
        <div hidden={mode.kind !== "capture"}>
          <Composer
            autoFocus={mode.kind === "capture"}
            onSaved={onSaved}
            onNotice={onNotice}
            onDraftChange={mode.kind === "capture" ? onDraftChange : undefined}
            onClose={closeComposer}
          />
        </div>
        {mode.kind === "edit" && (
          <Composer
            key={`${mode.note.id}:${mode.note.revision}`}
            note={mode.note}
            autoFocus
            onSaved={() => {
              closeComposer();
              onSaved();
            }}
            onNotice={onNotice}
            onDraftChange={onDraftChange}
            onClose={closeComposer}
            onReload={() => void reloadNote(mode.note.id)}
          />
        )}
      </div>
    </section>
  );
}

interface CardProps {
  note: V2Note;
  projects: V2Project[];
  editing: boolean;
  editorOpen: boolean;
  selectedTaskId: string | null;
  fresh: boolean;
  freshTasks: ReadonlySet<string>;
  onEdit: () => void;
  onOpenTask: (id: string) => void;
  onSaved: () => void;
}

function NoteCard({
  note,
  projects,
  editing,
  editorOpen,
  selectedTaskId,
  fresh,
  freshTasks,
  onEdit,
  onOpenTask,
  onSaved,
}: CardProps) {
  const active = new Set(note.lines.map((line) => line.id));
  // A removed source line has no active row; its Task still belongs to this Note and is
  // drawn from the frozen original line rather than disappearing with the edit.
  const detached = note.tasks.filter((task) => !active.has(task.sourceLineId));

  const renderTask = (task: V2Task) => (
    <div className="nb-task-line" key={task.id}>
      <TaskBranch
        task={task}
        project={projects.find((candidate) => candidate.id === task.projectId)}
        selected={task.id === selectedTaskId}
        fresh={freshTasks.has(task.id)}
        onOpen={() => onOpenTask(task.id)}
        detached={!active.has(task.sourceLineId)}
      />
    </div>
  );

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
        <div className="nb-note-actions">
          <button type="button" className="btn ghost nb-edit" onClick={onEdit} disabled={editorOpen}>
            {editing ? "Editing" : "Edit"}
          </button>
          {!editing && (
            <DeleteConfirmation
              label="Delete note"
              description={note.tasks.length > 0 ? "Delete this note’s tasks first." : "This permanently removes the note from Relay."}
              blocked={note.tasks.length > 0}
              onDelete={async () => {
                await v2NoteDelete(note.id);
                onSaved();
              }}
            />
          )}
        </div>
      </div>

      <div className="nb-lines">
        {note.lines.map((line) => {
          const tasks = note.tasks.filter((task) => task.sourceLineId === line.id);
          return (
            <Fragment key={line.id}>
              {!tasks.some((task) => task.originalLine === line.text) && (
                <div className="nb-line">
                  <div className="nb-line-text"><InlineLine text={line.text} references={line.references} /></div>
                </div>
              )}
              {tasks.map(renderTask)}
            </Fragment>
          );
        })}
        {detached.map(renderTask)}
      </div>
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
  const { originalLine } = task;
  const sourceMode = lineDirective(originalLine);
  const content = sourceMode ? originalLine.slice(SIGIL[sourceMode].length).trimStart() : originalLine;
  return (
    <button
      type="button"
      className={`nb-branch mode-${task.agentMode}${selected ? " on" : ""}${detached ? " detached" : ""}${fresh ? " fresh" : ""}`}
      data-task-id={task.id}
      data-tone={statusTone(task.status)}
      onClick={onOpen}
      title={detached ? "Source line removed; showing the saved Task line" : "Open Task"}
    >
      <span className="nb-sigil">{SIGIL[task.agentMode]}</span>
      <span className="nb-branch-text"><InlineLine text={content} references={task.references} /></span>
      <span className="nb-status">{task.status.replace(/_/g, " ")}</span>
      {project && <span className="nb-project">+{project.name}</span>}
    </button>
  );
}
