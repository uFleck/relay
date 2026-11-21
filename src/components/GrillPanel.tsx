import { useCallback, useEffect, useRef, useState } from "react";
import {
  errorText,
  onEvent,
  v2GrillingAnswer,
  v2GrillingCancel,
  v2GrillingContinueLinkOnly,
  v2GrillingGo,
  v2GrillingRetry,
  v2GrillingStart,
  v2GrillingView,
  v2TaskAssignProject,
  type V2GrillView,
  type V2Project,
} from "../ipc";

/* One inline Grilling panel shared by the main-window Task sheet and the Dock. It owns only
   presentation: every gate, the transcript and the closing approval live in relay-core. */

interface Props {
  taskId: string;
  projects: V2Project[];
  compact?: boolean;
  onNotice?: (message: string) => void;
  onChanged?: () => void;
}

export function GrillPanel({ taskId, projects, compact = false, onNotice, onChanged }: Props) {
  const [view, setView] = useState<V2GrillView | null>(null);
  const [draft, setDraft] = useState("");
  const [answer, setAnswer] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const started = useRef(false);

  const apply = useCallback((next: V2GrillView | null | void) => {
    if (!next) return;
    setView(next);
    setDraft((current) => (current.trim() ? current : next.rendered ?? ""));
  }, []);

  const refresh = useCallback(async () => {
    try {
      apply(await v2GrillingView(taskId));
    } catch (cause) {
      setError(errorText(cause));
    }
  }, [taskId, apply]);

  useEffect(() => {
    started.current = false;
    setDraft("");
    setAnswer("");
    void refresh();
    const offGrill = onEvent("relay://grilling", (id) => {
      if (!id || id === taskId) void refresh();
    });
    const offChange = onEvent("relay://v2-change", () => void refresh());
    return () => {
      offGrill();
      offChange();
    };
  }, [taskId, refresh]);

  // A newly submitted ?? Task starts its own exchange. Core collapses a competing request.
  useEffect(() => {
    if (!view || started.current || view.revision !== null) return;
    started.current = true;
    void run(async () => {
      await v2GrillingStart(taskId);
      return null;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view?.revision, taskId]);

  async function run(action: () => Promise<V2GrillView | null | void>) {
    setBusy(true);
    setError(null);
    try {
      const next = await action();
      apply(next);
      onChanged?.();
    } catch (cause) {
      const text = errorText(cause);
      setError(text);
      onNotice?.(text);
    } finally {
      setBusy(false);
    }
  }

  const go = () => {
    if (!view?.canGo || view.revision === null) return;
    void run(async () => apply(await v2GrillingGo(taskId, view.revision!, draft)));
  };

  // Ctrl+Enter inside the focused closing panel is Go. It never submits the Composer.
  const onKeyDown = (event: React.KeyboardEvent) => {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter" && view?.canGo) {
      event.preventDefault();
      event.stopPropagation();
      go();
    }
  };

  if (!view) {
    return <div className={`grill-panel${compact ? " compact" : ""}`}>Loading Grilling…</div>;
  }

  const ready = view.status === "brief";
  const failure = view.failureReason ?? view.failureStage;

  return (
    <div
      className={`grill-panel${compact ? " compact" : ""}`}
      data-task-id={taskId}
      data-status={view.taskStatus}
      data-turn={view.status ?? ""}
      data-ready={view.dispatchReady ? "true" : "false"}
      onKeyDown={onKeyDown}
    >
      <div className="brief-head">
        <span className="tag mode-grill">grill</span>
        <span className="meta">Exchange{view.revision ? ` · rev ${view.revision}` : ""}</span>
        <span className="push" />
        <span className="meta">{view.taskStatus.replace(/_/g, " ")}</span>
      </div>

      {view.projectId === null && (
        <label className="brief-row">
          <span>Project</span>
          <select
            aria-label="Project for this Task"
            value=""
            disabled={busy}
            onChange={(event) =>
              event.target.value &&
              void run(async () => {
                await v2TaskAssignProject(taskId, event.target.value);
                return await v2GrillingView(taskId);
              })
            }
          >
            <option value="">Choose project…</option>
            {projects.map((project) => (
              <option key={project.id} value={project.id}>
                {project.name}
              </option>
            ))}
          </select>
        </label>
      )}

      {view.entries.length > 0 && (
        <ol className="grill-transcript" aria-label="Saved Grilling exchange">
          {view.entries.map((entry, index) => (
            <li key={`${entry.createdAt}-${index}`} className={`grill-${entry.role}`}>
              <span className="meta mono">{entry.role}</span>
              <span className="grill-line">{entry.text}</span>
            </li>
          ))}
        </ol>
      )}
      {view.exchanges.length > 1 && (
        <p className="meta">Earlier exchanges kept on this Task: {view.exchanges.length - 1}</p>
      )}

      {view.canAnswer && (
        <form
          className="brief-row"
          onSubmit={(event) => {
            event.preventDefault();
            if (!answer.trim() || view.revision === null) return;
            void run(async () => {
              const next = await v2GrillingAnswer(taskId, view.revision!, answer);
              setAnswer("");
              return next;
            });
          }}
        >
          <input
            aria-label="Your answer"
            placeholder="Your answer…"
            value={answer}
            disabled={busy}
            onChange={(event) => setAnswer(event.target.value)}
          />
          <button type="submit" className="btn pri" disabled={busy || !answer.trim()}>
            Answer
          </button>
        </form>
      )}

      {view.pendingAction && view.entries.length === 0 && (
        <p className="brief-wait">{view.pendingAction}</p>
      )}

      {view.missingReferences.length > 0 && ready && (
        <div className="brief-missing">
          <p className="meta">Content unavailable for these References:</p>
          {view.missingReferences.map((identity) => (
            <button
              type="button"
              key={identity}
              disabled={busy || view.revision === null}
              onClick={() =>
                void run(async () =>
                  apply(
                    await v2GrillingContinueLinkOnly(taskId, view.revision!, identity),
                  )
                )
              }
            >
              Continue with link only · {identity}
            </button>
          ))}
        </div>
      )}

      {ready && (
        <>
          {view.rendered === null ? (
            <p className="brief-wait">Writing the Brief…</p>
          ) : (
            <textarea
              className="brief-text"
              aria-label="Editable Grilling Brief"
              value={draft}
              disabled={busy}
              onChange={(event) => setDraft(event.target.value)}
            />
          )}
          <div className="brief-actions">
            <button
              type="button"
              className="btn pri"
              disabled={busy || !view.canGo}
              onClick={go}
            >
              Go · approve Brief <kbd>Ctrl ↵</kbd>
            </button>
          </div>
        </>
      )}

      {failure && <p className="brief-error">{failure}</p>}
      {error && <p className="brief-error">{error}</p>}

      <div className="brief-actions">
        {view.canRetry && (
          <button type="button" className="btn" disabled={busy} onClick={() => void run(async () => apply(await v2GrillingRetry(taskId)))}>
            Retry
          </button>
        )}
        {view.canCancel && (
          <button type="button" className="btn" disabled={busy} onClick={() => void run(async () => apply(await v2GrillingCancel(taskId)))}>
            Cancel
          </button>
        )}
        {view.dispatchReady && <span className="brief-ready">Ready for dispatch</span>}
      </div>
    </div>
  );
}
