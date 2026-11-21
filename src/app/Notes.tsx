import { useCallback, useEffect, useRef, useState } from "react";
import { Icon } from "../components/Icon";
import { createNote, deleteNote, errorText, listNotes, noteToTask, openT3, setNotePinned, updateNote, type Note } from "../ipc";
import { composePrompt, noteLabel, PROMPT_TEMPLATES, sortNotes, type PromptTemplate } from "../notes";
import type { Project } from "../types";
import { Select } from "../ui/controls";
import { V2Capture } from "./V2Capture";

/* Notes: a list and an editor. Edits save on their own after a short pause; switching notes, leaving the
   view or reloading saves first. Saves run one after another so an older text never lands last. */

const SAVE_DELAY_MS = 600;
const SEARCH_DELAY_MS = 150;

type Draft = Pick<Note, "id" | "title" | "body">;
/** The open note. `pinned` lives here too, so a search that hides the row keeps the pin button right. */
type Open = Draft & Pick<Note, "pinned">;
type SaveState = "saved" | "saving" | "error";

interface Props {
  projects: Project[];
  onNotice: (text: string) => void;
  /** A task was created from a note; the app refreshes its snapshot. */
  onTaskCreated: (task: { id: string; title: string }) => void;
}

export function NotesView({ projects, onNotice, onTaskCreated }: Props) {
  const [notes, setNotes] = useState<Note[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [query, setQuery] = useState("");
  const [draft, setDraft] = useState<Open | null>(null);
  const [saveState, setSaveState] = useState<SaveState>("saved");
  const [template, setTemplate] = useState<PromptTemplate>("");
  const [projectId, setProjectId] = useState(projects[0]?.id ?? "");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState(false);
  const titleRef = useRef<HTMLInputElement>(null);
  const pending = useRef<Draft | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const queue = useRef<Promise<unknown>>(Promise.resolve());

  /** Writes the pending edit now. Resolves once every queued save has finished. */
  const flush = useCallback(() => {
    window.clearTimeout(timer.current);
    const edit = pending.current;
    pending.current = null;
    if (edit) {
      setSaveState("saving");
      queue.current = queue.current
        .then(() => updateNote(edit))
        .then((saved) => {
          setNotes((list) => sortNotes(list.map((n) => (n.id === saved.id ? { ...saved, title: n.title, body: n.body } : n))));
          if (!pending.current) setSaveState("saved");
        })
        .catch((error) => {
          // Keep the text queued so the next flush (a switch, leaving, a reload) tries again.
          pending.current ??= edit;
          setSaveState("error");
          onNotice(`Could not save the note: ${errorText(error)}`);
        });
    }
    return queue.current;
  }, [onNotice]);

  useEffect(() => {
    const save = () => void flush();
    window.addEventListener("beforeunload", save);
    return () => {
      window.removeEventListener("beforeunload", save);
      save();
    };
  }, [flush]);

  useEffect(() => {
    let gone = false;
    const id = window.setTimeout(() => {
      listNotes(query)
        .then((list) => {
          if (gone) return;
          // A row with an unsaved edit keeps the text being typed.
          const edit = pending.current;
          setNotes(edit ? list.map((n) => (n.id === edit.id ? { ...n, title: edit.title, body: edit.body } : n)) : list);
        })
        .catch((error) => onNotice(`Could not load notes: ${errorText(error)}`))
        .finally(() => !gone && setLoaded(true));
    }, query ? SEARCH_DELAY_MS : 0);
    return () => {
      gone = true;
      window.clearTimeout(id);
    };
  }, [query, onNotice]);

  useEffect(() => {
    if (!projects.some((p) => p.id === projectId)) setProjectId(projects[0]?.id ?? "");
  }, [projects, projectId]);

  const edit = (patch: Partial<Pick<Note, "title" | "body">>) => {
    if (!draft) return;
    const next = { ...draft, ...patch };
    setDraft(next);
    // The list label follows the typing; the saved row replaces it after the write.
    setNotes((list) => list.map((n) => (n.id === next.id ? { ...n, ...patch } : n)));
    pending.current = { id: next.id, title: next.title, body: next.body };
    setSaveState("saving");
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => void flush(), SAVE_DELAY_MS);
  };

  const open = (note: Note) => {
    if (note.id === draft?.id) return;
    void flush();
    setConfirmDelete(false);
    setDraft({ id: note.id, title: note.title, body: note.body, pinned: note.pinned });
  };

  const create = async () => {
    void flush();
    try {
      const note = await createNote();
      setQuery("");
      setNotes((list) => sortNotes([note, ...list.filter((n) => n.id !== note.id)]));
      setConfirmDelete(false);
      setDraft({ id: note.id, title: note.title, body: note.body, pinned: note.pinned });
      requestAnimationFrame(() => titleRef.current?.focus());
    } catch (error) {
      onNotice(`Could not create a note: ${errorText(error)}`);
    }
  };

  const togglePin = async () => {
    if (!draft) return;
    try {
      const saved = await setNotePinned(draft.id, !draft.pinned);
      setDraft((d) => (d?.id === saved.id ? { ...d, pinned: saved.pinned } : d));
      setNotes((list) => sortNotes(list.map((n) => (n.id === saved.id ? { ...n, pinned: saved.pinned } : n))));
    } catch (error) {
      onNotice(`Could not pin the note: ${errorText(error)}`);
    }
  };

  const remove = async () => {
    if (!draft) return;
    const id = draft.id;
    window.clearTimeout(timer.current);
    const kept = pending.current?.id === id ? pending.current : null;
    if (kept) pending.current = null;
    setBusy(true);
    try {
      await queue.current;
      await deleteNote(id);
      setNotes((list) => list.filter((n) => n.id !== id));
      setDraft(null);
      setSaveState("saved");
    } catch (error) {
      if (kept) pending.current ??= kept;
      onNotice(`Could not delete the note: ${errorText(error)}`);
    } finally {
      setBusy(false);
      setConfirmDelete(false);
    }
  };

  const sendToT3 = async () => {
    if (!draft) return;
    const prompt = composePrompt(draft, template);
    if (!prompt) {
      onNotice("Write something in the note first.");
      return;
    }
    try {
      await navigator.clipboard.writeText(prompt);
    } catch (error) {
      onNotice(`Could not copy the prompt: ${errorText(error)}`);
      return;
    }
    try {
      await openT3();
      onNotice("Prompt copied. Paste it into a T3 Code thread.");
    } catch (error) {
      onNotice(`Prompt copied, but T3 Code did not open: ${errorText(error)}`);
    }
  };

  const toTask = async () => {
    if (!draft || !projectId) return;
    setBusy(true);
    try {
      await flush();
      const task = await noteToTask(draft.id, projectId);
      onTaskCreated(task);
      onNotice(`Task created: ${task.title}. Agents can claim it over MCP.`);
    } catch (error) {
      onNotice(`Could not create the task: ${errorText(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const sub = query ? `${notes.length} matching` : `${notes.length} ${notes.length === 1 ? "note" : "notes"}`;

  return (
    <section className="main notes">
      <V2Capture onNotice={onNotice} />
      <header className="head">
        <div>
          <h1>Notes</h1>
          <div className="date meta">{sub}</div>
        </div>
        <button type="button" className="btn pri push" onClick={() => void create()}>
          New note
        </button>
      </header>
      <div className="notes-body">
        <div className="notes-list">
          <label className="search notes-search">
            <Icon name="search" />
            <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search notes" aria-label="Search notes" />
          </label>
          <div className="scroll" role="list">
            {loaded && notes.length === 0 && (
              <p className="empty">{query ? "No note matches." : "No notes yet. Start one with New note."}</p>
            )}
            {notes.map((n) => (
              <button
                type="button"
                role="listitem"
                key={n.id}
                className={`note-row${n.id === draft?.id ? " on" : ""}`}
                aria-current={n.id === draft?.id || undefined}
                onClick={() => open(n)}
              >
                <span className="note-t">
                  {n.pinned && <Icon name="pin" />}
                  {noteLabel(n)}
                </span>
                <span className="meta ellip">{n.title.trim() ? n.body.trim().split("\n")[0] : ""}</span>
              </button>
            ))}
          </div>
        </div>

        {draft ? (
          <div className="note-ed">
            <div className="note-bar">
              <button
                type="button"
                className={`ib${draft.pinned ? " on" : ""}`}
                title={draft.pinned ? "Unpin" : "Pin"}
                aria-pressed={draft.pinned}
                onClick={() => void togglePin()}
              >
                <Icon name="pin" />
              </button>
              <span className={`meta${saveState === "error" ? " warn-fg" : ""}`} role="status">
                {saveState === "saving" ? "Saving…" : saveState === "error" ? "Not saved" : "Saved"}
              </span>
              <span className="push" />
              {confirmDelete ? (
                <>
                  <span className="meta">Delete this note?</span>
                  <button type="button" className="btn bad" disabled={busy} onClick={() => void remove()}>
                    Delete
                  </button>
                  <button type="button" className="btn ghost" onClick={() => setConfirmDelete(false)}>
                    Cancel
                  </button>
                </>
              ) : (
                <button type="button" className="btn ghost" onClick={() => setConfirmDelete(true)}>
                  Delete
                </button>
              )}
            </div>
            <input
              ref={titleRef}
              className="note-title"
              value={draft.title}
              placeholder="Title"
              aria-label="Note title"
              onChange={(e) => edit({ title: e.target.value })}
            />
            <textarea
              className="note-text"
              value={draft.body}
              placeholder="Write the note…"
              aria-label="Note text"
              onChange={(e) => edit({ body: e.target.value })}
            />
            <div className="note-actions">
              <Select value={template} options={PROMPT_TEMPLATES} onChange={setTemplate} />
              <button type="button" className="btn" onClick={() => void sendToT3()} title="Copies the prompt and opens T3 Code">
                <Icon name="ext" />
                Send to T3
              </button>
              <span className="push" />
              {projects.length > 0 ? (
                <>
                  <Select value={projectId} options={projects.map((p) => ({ value: p.id, label: p.name }))} onChange={setProjectId} />
                  <button type="button" className="btn" disabled={busy || !projectId} onClick={() => void toTask()}>
                    <Icon name="check" />
                    Turn into task
                  </button>
                </>
              ) : (
                <span className="meta">Add a project to turn notes into tasks.</span>
              )}
            </div>
          </div>
        ) : (
          <div className="note-ed note-none">
            <p className="empty">Pick a note, or start a new one.</p>
          </div>
        )}
      </div>
    </section>
  );
}
