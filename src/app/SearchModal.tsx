import { useEffect, useMemo, useRef, useState } from "react";
import { Icon } from "../components/Icon";
import type { V2WorkspaceSnapshot } from "../ipc";
import type { WorkspaceFilter } from "../workspace";

/* Ctrl K opens Search at once with its own input. The views behind it stay unfiltered, so
   closing the modal leaves the Notebook and Tasks as they were. */

interface Props {
  query: string;
  onQuery: (query: string) => void;
  filter: WorkspaceFilter;
  workspace: V2WorkspaceSnapshot;
  onOpenNote: (id: string) => void;
  onOpenTask: (id: string) => void;
  onClose: () => void;
}

interface Hit {
  key: string;
  kind: "note" | "task";
  id: string;
  title: string;
  meta: string;
}

function shortDate(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleDateString();
}

export function SearchModal({ query, onQuery, filter, workspace, onOpenNote, onOpenTask, onClose }: Props) {
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const searching = query.trim().length > 0;

  const hits = useMemo<Hit[]>(() => {
    const notes = workspace.notes
      .filter((note) => filter.noteIds?.has(note.id))
      .map((note) => ({
        key: `note:${note.id}`,
        kind: "note" as const,
        id: note.id,
        title: note.body.split("\n")[0]?.trim() || "(empty note)",
        meta: `${note.tasks.length} task${note.tasks.length === 1 ? "" : "s"} · ${shortDate(note.createdAt)}`,
      }));
    const tasks = workspace.tasks
      .filter((task) => filter.taskIds?.has(task.id))
      .map((task) => ({
        key: `task:${task.id}`,
        kind: "task" as const,
        id: task.id,
        title: task.originalLine.trim() || task.brief?.trim() || "(task)",
        meta: `${task.agentMode} · ${task.status.replace(/_/g, " ")}`,
      }));
    return [...notes, ...tasks];
  }, [workspace, filter.noteIds, filter.taskIds]);

  useEffect(() => setActive(0), [query]);
  useEffect(() => input.current?.focus(), []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        event.stopPropagation();
        input.current?.focus();
        input.current?.select();
      } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setActive((current) => {
          if (!hits.length) return 0;
          const step = event.key === "ArrowDown" ? 1 : -1;
          return (current + step + hits.length) % hits.length;
        });
      } else if (event.key === "Enter" && searching && hits[active]) {
        event.preventDefault();
        open(hits[active]);
      } else if (event.key === "Escape") {
        // The modal owns Escape while it is open; the shell must not also close a Task sheet.
        event.preventDefault();
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [hits, active, onClose, searching]);

  function open(hit: Hit) {
    if (hit.kind === "note") onOpenNote(hit.id);
    else onOpenTask(hit.id);
    onClose();
  }

  return (
    <div className="search-shade" onMouseDown={(event) => {
      if (event.target === event.currentTarget) onClose();
    }}>
      <section className="search-modal" role="dialog" aria-modal="true" aria-label="Search results">
        <header className="sm-head">
          <Icon name="search" small />
          <input
            ref={input}
            className="sm-input"
            value={query}
            onChange={(event) => onQuery(event.target.value)}
            placeholder="Search notes and tasks"
            spellCheck={false}
            aria-label="Search notes and tasks"
          />
          {searching && (
            <span className="meta">
              {filter.ready ? `${hits.length} result${hits.length === 1 ? "" : "s"}` : "Searching…"}
            </span>
          )}
          <button type="button" className="ib" title="Close (Esc)" onClick={onClose}>
            <Icon name="x" small />
          </button>
        </header>
        <div className="sm-list">
          {!searching && <p className="empty">Type to search notes and tasks.</p>}
          {searching && filter.ready && hits.length === 0 && <p className="empty">No notes or tasks match “{query}”.</p>}
          {searching && hits.map((hit, index) => (
            <button
              type="button"
              key={hit.key}
              className={`sm-hit${index === active ? " on" : ""}`}
              onMouseEnter={() => setActive(index)}
              onClick={() => open(hit)}
            >
              <span className={`sm-kind ${hit.kind}`}>{hit.kind === "note" ? "Note" : "Task"}</span>
              <span className="sm-title">{hit.title}</span>
              <span className="sm-meta meta">{hit.meta}</span>
            </button>
          ))}
        </div>
        <footer className="sm-foot meta">
          <span><kbd>↑</kbd><kbd>↓</kbd> move</span>
          <span><kbd>↵</kbd> open</span>
          <span><kbd>Esc</kbd> close</span>
        </footer>
      </section>
    </div>
  );
}
