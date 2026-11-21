import { useCallback, useEffect, useRef, useState } from "react";
import { EMPTY_DRAFT, type DraftSummary } from "./directives";
import { TitleBar } from "./app/TitleBar";
import { Rail } from "./app/Rail";
import { Notebook } from "./app/Notebook";
import { MissingTask, Tasks } from "./app/Tasks";
import { TaskSheet } from "./app/TaskSheet";
import { SettingsView } from "./app/Settings";
import { errorText, getSnapshot, onEvent } from "./ipc";
import type { Snapshot } from "./types";
import { parseRoute, readLocation, writeLocation, type Route, type View } from "./route";
import { startUpdateChecks, useUpdater } from "./updater";
import { needsYouCount, useWorkspace, useWorkspaceFilter } from "./workspace";

const typing = (el: EventTarget | null) =>
  el instanceof HTMLElement && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName));

export default function App() {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [route, setRoute] = useState<Route>(readLocation);
  /** The raw filter query lives in the shell, so rail navigation never clears it. */
  const [query, setQuery] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [draft, setDraft] = useState<DraftSummary>(EMPTY_DRAFT);
  const update = useUpdater();
  const searchRef = useRef<HTMLInputElement>(null);
  const { workspace, version: workspaceVersion, error: workspaceError, refresh } = useWorkspace();
  const filter = useWorkspaceFilter(query, workspaceVersion);

  const refreshV1 = useCallback(async () => {
    try {
      setSnap(await getSnapshot());
      setLoadError(null);
    } catch (error) {
      console.error("[relay] snapshot", error);
      setLoadError(errorText(error));
    }
  }, []);

  const navigate = useCallback((next: Route) => {
    writeLocation(next);
    setRoute(next);
  }, []);

  useEffect(() => {
    startUpdateChecks();
    void refreshV1();
    // Nothing in the legacy store pushes data yet; coming back to the window re-reads it.
    const onFocus = () => void refreshV1();
    window.addEventListener("focus", onFocus);
    const offOpen = onEvent("relay://open", (r) => navigate(parseRoute(r)));
    return () => {
      window.removeEventListener("focus", onFocus);
      offOpen();
    };
  }, [refreshV1, navigate]);

  /** A Task sheet is always the `/tasks?task=<id>` route, even when opened from Notebook. */
  const openTask = useCallback(
    (task: string) => navigate({ view: "tasks", task, provider: null }),
    [navigate],
  );

  const closeSheet = useCallback(() => {
    navigate({ view: "tasks", task: null, provider: null });
    // Tasks puts focus back on the row that opened the sheet or on its heading.
  }, [navigate]);

  const setView = useCallback(
    (view: View) => navigate({ view, task: null, provider: null }),
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
      if (e.key === "Escape") {
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
        <div className="boot" role={workspaceError || loadError ? "alert" : "status"}>
          {workspaceError || loadError ? (
            <>
              <b>Relay could not read its workspace.</b>
              <span className="meta">{workspaceError ?? loadError}</span>
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

  const version = snap?.version ?? "";
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
          notice={<UpdateNotice update={update} />}
          wordmarkMode={draft.mode}
        />

        {route.view === "notebook" && (
          <Notebook
            workspace={workspace}
            filter={filter}
            selectedTaskId={route.task}
            onOpenTask={openTask}
            onNotice={setNotice}
            onSaved={afterSave}
            onDraftChange={setDraft}
          />
        )}
        {route.view === "tasks" && (
          <Tasks workspace={workspace} filter={filter} selectedTaskId={route.task} onSelect={openTask} />
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
            onClose={closeSheet}
          />
        )}
        {route.task && !selectedTask && <MissingTask id={route.task} onClose={closeSheet} />}
      </div>
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

/** Kept local so the rail's notice and the Settings view share one updater instance. */
function UpdateNotice({ update }: { update: ReturnType<typeof useUpdater> }) {
  if (update.kind !== "available" && update.kind !== "downloading" && update.kind !== "ready") return null;
  return (
    <div className="unote" role="status">
      <span>
        <b>{update.version}</b>{" "}
        {update.kind === "ready"
          ? "ready"
          : update.kind === "available"
            ? "available"
            : `downloading${update.progress === null ? "…" : ` ${update.progress}%`}`}
      </span>
    </div>
  );
}
