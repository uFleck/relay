import { useCallback, useEffect, useRef, useState } from "react";
import { EMPTY_DRAFT, type DraftSummary } from "./directives";
import { TitleBar } from "./app/TitleBar";
import { Rail } from "./app/Rail";
import { Notebook } from "./app/Notebook";
import { MissingTask, Tasks } from "./app/Tasks";
import { TaskSheet } from "./app/TaskSheet";
import { SettingsView } from "./app/Settings";
import { SearchModal } from "./app/SearchModal";
import { UpdateActions } from "./app/UpdateActions";
import { appVersion, onEvent } from "./ipc";
import { parseRoute, readLocation, writeLocation, type Route, type View } from "./route";
import { startUpdateChecks, useUpdater } from "./updater";
import { needsYouCount, useWorkspace, useWorkspaceFilter } from "./workspace";

const typing = (el: EventTarget | null) =>
  el instanceof HTMLElement && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName));

export default function App() {
  const [version, setVersion] = useState("");
  const [route, setRoute] = useState<Route>(readLocation);
  /** The raw filter query lives in the shell, so rail navigation never clears it. */
  const [query, setQuery] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [draft, setDraft] = useState<DraftSummary>(EMPTY_DRAFT);
  const update = useUpdater();
  const searchRef = useRef<HTMLInputElement>(null);
  const { workspace, version: workspaceVersion, error: workspaceError, refresh } = useWorkspace();
  const filter = useWorkspaceFilter(query, workspaceVersion);
  /** Search owns the query. The views keep showing the full workspace behind the modal. */
  const viewFilter = useWorkspaceFilter("", workspaceVersion);

  const navigate = useCallback((next: Route) => {
    writeLocation(next);
    setRoute(next);
  }, []);

  useEffect(() => {
    startUpdateChecks();
    void appVersion().then(setVersion).catch(() => undefined);
    const offOpen = onEvent("relay://open", (r) => navigate(parseRoute(r)));
    return () => offOpen();
  }, [navigate]);

  /** A Task sheet is always the `/tasks?task=<id>` route, even when opened from Notebook. */
  const openTask = useCallback(
    (task: string) => navigate({ view: "tasks", task, note: null, provider: null }),
    [navigate],
  );

  /** A search hit opens the Note in the Notebook with its detail editor. */
  const openNote = useCallback(
    (note: string) => navigate({ view: "notebook", task: null, note, provider: null }),
    [navigate],
  );

  const closeSheet = useCallback(() => {
    navigate({ view: "tasks", task: null, note: null, provider: null });
    // Tasks puts focus back on the row that opened the sheet or on its heading.
  }, [navigate]);

  const setView = useCallback(
    (view: View) => navigate({ view, task: null, note: null, provider: null }),
    [navigate],
  );

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
        return;
      }
      if (e.key === "Escape" && !e.defaultPrevented && !document.querySelector(".delete-confirmation")) {
        if (route.task && !typing(e.target)) {
          e.preventDefault();
          closeSheet();
        } else if (notice) setNotice(null);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [route.task, notice, closeSheet]);

  if (!workspace) {
    return (
      <div className="win">
        <TitleBar ref={searchRef} query={query} onQuery={setQuery} />
        <div className="boot" role={workspaceError ? "alert" : "status"}>
          {workspaceError ? (
            <>
              <b>Relay could not read its workspace.</b>
              <span className="meta">{workspaceError}</span>
              <button type="button" className="btn" onClick={() => void refresh()}>
                Try again
              </button>
            </>
          ) : (
            <span className="meta">Loading…</span>
          )}
        </div>
      </div>
    );
  }

  const selectedTask = route.task
    ? workspace.tasks.find((task) => task.id === route.task)
    : undefined;
  const needsYou = needsYouCount(workspace.tasks);
  const afterSave = () => void refresh();

  return (
    <div className="win">
      <TitleBar ref={searchRef} query={query} onQuery={setQuery} />
      <div className={`body${route.task ? " with-sheet" : ""}`}>
        <Rail
          view={route.view}
          onView={setView}
          version={version}
          needsYou={needsYou}
          notice={<RailUpdate update={update} />}
          wordmarkMode={draft.mode}
        />

        {route.view === "notebook" && (
          <Notebook
            workspace={workspace}
            filter={viewFilter}
            focusNoteId={route.note}
            selectedTaskId={route.task}
            onOpenTask={openTask}
            onNotice={setNotice}
            onSaved={afterSave}
            onDraftChange={setDraft}
          />
        )}
        {route.view === "tasks" && (
          <Tasks workspace={workspace} filter={viewFilter} selectedTaskId={route.task} onSelect={openTask} />
        )}
        {route.view === "settings" && (
          <SettingsView provider={route.provider} version={version} update={update} />
        )}

        {route.task && selectedTask && (
          <TaskSheet
            key={selectedTask.id}
            task={selectedTask}
            project={workspace.projects.find((candidate) => candidate.id === selectedTask.projectId)}
            note={workspace.notes.find((candidate) => candidate.id === selectedTask.noteId)}
            projects={workspace.projects}
            onClose={closeSheet}
            onDeleted={() => {
              closeSheet();
              afterSave();
            }}
          />
        )}
        {route.task && !selectedTask && <MissingTask id={route.task} onClose={closeSheet} />}
      </div>
      {query.trim() && (
        <SearchModal
          query={query}
          filter={filter}
          workspace={workspace}
          onOpenNote={openNote}
          onOpenTask={openTask}
          onClose={() => setQuery("")}
        />
      )}
      {notice && (
        <div className="toast" role="alert">
          <span>{notice}</span>
          <button type="button" className="ib" title="Dismiss (Esc)" onClick={() => setNotice(null)}>
            ×
          </button>
        </div>
      )}
    </div>
  );
}

/** Updater lives in the rail: one line of state and the action that fits it. */
function RailUpdate({ update }: { update: ReturnType<typeof useUpdater> }) {
  if (update.kind === "idle" || update.kind === "checking") return null;
  const label =
    update.kind === "available"
      ? `${update.version} available`
      : update.kind === "downloading"
        ? `Downloading${update.progress === null ? "…" : ` ${update.progress}%`}`
        : update.kind === "ready"
          ? `${update.version} ready`
          : "Update failed";
  return (
    <div className="unote" role="status">
      <span className="tx">{label}</span>
      <UpdateActions update={update} />
    </div>
  );
}
