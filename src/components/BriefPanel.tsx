import { useCallback, useEffect, useRef, useState } from "react";
import {
  errorText,
  onEvent,
  v2BriefingApprove,
  v2BriefingContinueLinkOnly,
  v2BriefingGenerate,
  v2BriefingRetry,
  v2BriefingRewrite,
  v2BriefingView,
  v2TaskAssignProject,
  type V2BriefView,
  type V2Project,
} from "../ipc";

/* One inline Brief panel, shared by the main-window Task sheet and the Dock. It owns only
   presentation and the six thin commands; the core owns gates, revisions and approval races. */

interface Props {
  taskId: string;
  projects: V2Project[];
  compact?: boolean;
  onNotice?: (message: string) => void;
  onChanged?: () => void;
}

export function BriefPanel({ taskId, projects, compact = false, onNotice, onChanged }: Props) {
  const [view, setView] = useState<V2BriefView | null>(null);
  const [draft, setDraft] = useState("");
  const [instruction, setInstruction] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const panel = useRef<HTMLDivElement>(null);
  const started = useRef(false);

  const apply = useCallback((next: V2BriefView | null | void) => {
    if (!next) return;
    setView(next);
    setDraft((current) => (current.trim() ? current : next.rendered ?? ""));
  }, []);

  const refresh = useCallback(async () => {
    try {
      apply(await v2BriefingView(taskId));
    } catch (cause) {
      setError(errorText(cause));
    }
  }, [taskId, apply]);

  useEffect(() => {
    started.current = false;
    setDraft("");
    void refresh();
    const offBrief = onEvent("relay://briefing", (id) => {
      if (!id || id === taskId) void refresh();
    });
    const offChange = onEvent("relay://v2-change", () => void refresh());
    return () => {
      offBrief();
      offChange();
    };
  }, [taskId, refresh]);

  // A newly submitted Task starts its own generation. The core collapses a second request.
  useEffect(() => {
    if (!view || started.current) return;
    const fresh = view.briefRevision === null && view.taskStatus === "briefing";
    if (!fresh) return;
    started.current = true;
    void run(async () => {
      await v2BriefingGenerate(taskId);
      return null;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view?.briefRevision, view?.taskStatus, taskId]);

  async function run(action: () => Promise<V2BriefView | null | void>) {
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

  const assign = (projectId: string) => {
    if (!projectId) return;
    void run(async () => {
      await v2TaskAssignProject(taskId, projectId);
      return await v2BriefingView(taskId);
    });
  };

  const go = () => {
    if (!view?.canGo || view.briefRevision === null) return;
    void run(async () => apply(await v2BriefingApprove(taskId, view.briefRevision!, draft)));
  };

  // Ctrl+Enter inside the focused approval panel is Go. It never submits the Composer.
  const onKeyDown = (event: React.KeyboardEvent) => {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter" && view?.canGo) {
      event.preventDefault();
      event.stopPropagation();
      go();
    }
  };

  if (!view) {
    return <div className={`brief-panel${compact ? " compact" : ""}`}>Loading Brief…</div>;
  }

  const isWait = view.mode === "wait";
  const canEdit = isWait && view.rendered !== null;
  const failure = view.failureReason ?? view.failureStage;

  return (
    <div
      ref={panel}
      className={`brief-panel${compact ? " compact" : ""} mode-${view.mode}`}
      data-task-id={taskId}
      data-status={view.taskStatus}
      data-ready={view.dispatchReady ? "true" : "false"}
      onKeyDown={onKeyDown}
    >
      <div className="brief-head">
        <span className={`tag mode-${view.mode}`}>{view.mode}</span>
        <span className="meta">Brief{view.briefRevision ? ` · rev ${view.briefRevision}` : ""}</span>
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
            onChange={(event) => assign(event.target.value)}
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

      {view.missingReferences.length > 0 && (
        <div className="brief-missing">
          <p className="meta">Content unavailable for these References:</p>
          {view.missingReferences.map((identity) => (
            <button
              type="button"
              key={identity}
              disabled={busy}
              onClick={() =>
                void run(async () =>
                  apply(
                    await v2BriefingContinueLinkOnly(taskId, view.briefRevision!, identity),
                  )
                )
              }
            >
              Continue with link only · {identity}
            </button>
          ))}
        </div>
      )}

      {view.rendered === null ? (
        <p className="brief-wait">{view.pendingAction ?? "Writing the Brief…"}</p>
      ) : canEdit ? (
        <textarea
          className="brief-text"
          aria-label="Editable Brief"
          value={draft}
          disabled={busy}
          onChange={(event) => setDraft(event.target.value)}
        />
      ) : (
        <pre className="brief-text readonly">{view.rendered}</pre>
      )}

      {failure && <p className="brief-error">{failure}</p>}
      {error && <p className="brief-error">{error}</p>}

      {view.references.length > 0 && (
        <div className="brief-refs">
          {view.references.map((reference) => (
            <a
              key={reference.identity}
              className={`brief-ref input-${reference.input}`}
              href={reference.canonicalUrl}
              target="_blank"
              rel="noreferrer"
              title={reference.identity}
            >
              {reference.title ?? reference.identity}
              {reference.input === "linkOnly" || reference.input === "missing"
                ? " · link only"
                : ""}
            </a>
          ))}
        </div>
      )}

      {isWait && view.canRewrite && (
        <div className="brief-row">
          <input
            aria-label="Rewrite instruction"
            placeholder="Rewrite instruction…"
            value={instruction}
            disabled={busy}
            onChange={(event) => setInstruction(event.target.value)}
          />
          <button
            type="button"
            className="btn"
            disabled={busy || !instruction.trim() || view.briefRevision === null}
            onClick={() =>
              void run(async () =>
                apply(
                  await v2BriefingRewrite(taskId, view.briefRevision!, instruction.trim()),
                )
              )
            }
          >
            Rewrite
          </button>
        </div>
      )}

      <div className="brief-actions">
        {view.canRetry && (
          <button type="button" className="btn" disabled={busy} onClick={() => void run(async () => apply(await v2BriefingRetry(taskId)))}>
            Retry
          </button>
        )}
        {isWait && (
          <button type="button" className="btn pri" disabled={busy || !view.canGo} onClick={go}>
            Go <kbd>Ctrl ↵</kbd>
          </button>
        )}
        {!isWait && view.dispatchReady && <span className="brief-ready">Ready for dispatch</span>}
      </div>
    </div>
  );
}
