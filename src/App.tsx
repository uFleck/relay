import { useCallback, useEffect, useMemo, useState } from "react";
import { EMPTY_DRAFT, type DraftSummary } from "./directives";
import { TitleBar } from "./app/TitleBar";
import { PageBarContext } from "./app/PageBar";
import { Rail } from "./app/Rail";
import { Notebook } from "./app/Notebook";
import { MissingTask, Tasks } from "./app/Tasks";
import { TaskSheet } from "./app/TaskSheet";
import { SettingsView } from "./app/Settings";
import { SearchModal } from "./app/SearchModal";
import { useVim, typing } from "./app/useVim";
import { VimHelp } from "./app/VimHelp";
import { useKeybinds } from "./keybinds";
import { appVersion, onEvent } from "./ipc";
import { parseRoute, readLocation, writeLocation, type Route, type View } from "./route";
import { startUpdateChecks, useUpdater } from "./updater";
import { needsYouCount, useWorkspace, useWorkspaceFilter } from "./workspace";

export default function App() {
  const [version, setVersion] = useState("");
  const [route, setRoute] = useState<Route>(readLocation);
  /** The raw filter query lives in the shell, so rail navigation never clears it. */
  const [query, setQuery] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [draft, setDraft] = useState<DraftSummary>(EMPTY_DRAFT);
  const update = useUpdater();
  const [searchOpen, setSearchOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [captureRequested, setCaptureRequested] = useState(false);
  const { vim } = useKeybinds();
  const [titleSlot, setTitleSlot] = useState<HTMLDivElement | null>(null);
  const [actionsSlot, setActionsSlot] = useState<HTMLDivElement | null>(null);
  const slots = useMemo(() => ({ title: titleSlot, actions: actionsSlot }), [titleSlot, actionsSlot]);
  const { workspace, version: workspaceVersion, error: workspaceError, refresh } = useWorkspace();
  const filter = useWorkspaceFilter(query, workspaceVersion);
  /** Search owns the query. The views keep showing the full workspace behind the modal. */
  const openSearch = useCallback(() => setSearchOpen(true), []);
  const closeSearch = useCallback(() => {
    setSearchOpen(false);
    setQuery("");
  }, []);
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

  const onVim = useCallback(
    (command: "newNote" | "goNotebook" | "goTasks" | "goSettings" | "search" | "help") => {
      if (command === "search") openSearch();
      else if (command === "help") setHelpOpen(true);
      else if (command === "goNotebook") setView("notebook");
      else if (command === "goTasks") setView("tasks");
      else if (command === "goSettings") setView("settings");
      else {
        setView("notebook");
        setCaptureRequested(true);
      }
    },
    [openSearch, setView],
  );
  const onCaptureHandled = useCallback(() => setCaptureRequested(false), []);
  const closeHelp = useCallback(() => setHelpOpen(false), []);
  const vimMode = useVim({
    enabled: vim && !!workspace,
    blocked: searchOpen || helpOpen,
    scope: route.view,
    onCommand: onVim,
  });

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        openSearch();
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
  }, [route.task, notice, closeSheet, openSearch]);

  if (!workspace) {
    return (
      <div className="win solo">
        <div className="column">
          <TitleBar />
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
      <Rail
        view={route.view}
        onView={setView}
        version={version}
        needsYou={needsYou}
        update={update}
        wordmarkMode={draft.mode}
        vimMode={vim ? vimMode : null}
      />
      <div className="column">
        <TitleBar onTitleSlot={setTitleSlot} onActionsSlot={setActionsSlot} onSearch={openSearch} />
        <PageBarContext.Provider value={slots}>
          <div className={`body${route.task ? " with-sheet" : ""}`}>
            {route.view === "notebook" && (
              <Notebook
                workspace={workspace}
                filter={viewFilter}
                focusNoteId={route.note}
                onCloseNote={() => setView("notebook")}
                selectedTaskId={route.task}
                onOpenTask={openTask}
                onNotice={setNotice}
                onSaved={afterSave}
                onDraftChange={setDraft}
                captureRequested={captureRequested}
                onCaptureHandled={onCaptureHandled}
              />
            )}
            {route.view === "tasks" && (
              <Tasks workspace={workspace} filter={viewFilter} selectedTaskId={route.task} onSelect={openTask} />
            )}
            {route.view === "settings" && (
              <SettingsView provider={route.provider} version={version} />
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
        </PageBarContext.Provider>
      </div>
      {searchOpen && (
        <SearchModal
          query={query}
          onQuery={setQuery}
          filter={filter}
          workspace={workspace}
          onOpenNote={openNote}
          onOpenTask={openTask}
          onClose={closeSearch}
        />
      )}
      {helpOpen && <VimHelp onClose={closeHelp} />}
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
