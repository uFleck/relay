import { useCallback, useEffect, useState } from "react";
import {
  errorText,
  onEvent,
  v2DispatchCheck,
  v2DispatchLink,
  v2DispatchRetry,
  v2DispatchView,
  type V2DispatchView,
} from "../ipc";

/* One inline dispatch panel for the Task sheet. It renders only what core stored: a known failure
   offers Retry after Settings recovers, and an uncertain outcome offers Check again and a manual
   link. Neither button can start a second thread while the first outcome is unresolved. */

interface Props {
  taskId: string;
  onNotice?: (message: string) => void;
  onChanged?: () => void;
}

const STATE_TEXT: Record<V2DispatchView["state"], string> = {
  launching: "Starting the T3 thread…",
  launch_failed: "T3 could not start the thread.",
  reconcile_required: "The start outcome is uncertain.",
  bound: "T3 thread linked.",
};

export function DispatchPanel({ taskId, onNotice, onChanged }: Props) {
  const [view, setView] = useState<V2DispatchView | null>(null);
  const [threadId, setThreadId] = useState("");
  const [compared, setCompared] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setView(await v2DispatchView(taskId));
    } catch (cause) {
      setError(errorText(cause));
    }
  }, [taskId]);

  useEffect(() => {
    void refresh();
    const offDispatch = onEvent("relay://dispatch", (id) => {
      if (!id || id === taskId) void refresh();
    });
    const offChange = onEvent("relay://v2-change", () => void refresh());
    return () => {
      offDispatch();
      offChange();
    };
  }, [taskId, refresh]);

  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      onChanged?.();
      await refresh();
    } catch (cause) {
      const text = errorText(cause);
      setError(text);
      onNotice?.(text);
    } finally {
      setBusy(false);
    }
  }

  if (!view) return null;

  return (
    <div className="dispatch-panel">
      <p className="desc">{STATE_TEXT[view.state]}</p>
      {view.failureReason && <p className="desc">{view.failureReason}</p>}
      {view.threadUrl && (
        <a className="lnk" href={view.threadUrl} target="_blank" rel="noreferrer">
          Open thread
        </a>
      )}

      {view.canRetry && (
        <>
          <p className="desc">
            Check T3 in Settings, then Retry. The Brief and approval are preserved.
          </p>
          <button type="button" className="btn" disabled={busy} onClick={() => void run(() => v2DispatchRetry(taskId))}>
            Retry
          </button>
        </>
      )}

      {view.canCheckAgain && (
        <>
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => void run(() => v2DispatchCheck(taskId))}
          >
            Check again
          </button>
          <div className="dispatch-link">
            <label className="desc" htmlFor={`dispatch-thread-${taskId}`}>
              Link existing thread
            </label>
            <input
              id={`dispatch-thread-${taskId}`}
              className="input"
              value={threadId}
              placeholder="T3 thread id"
              onChange={(event) => setThreadId(event.target.value)}
            />
            <label className="desc">
              <input
                type="checkbox"
                checked={compared}
                onChange={(event) => setCompared(event.target.checked)}
              />{" "}
              I compared its first message with the Brief in T3.
            </label>
            <button
              type="button"
              className="btn"
              disabled={busy || !threadId.trim() || !compared}
              onClick={() => void run(() => v2DispatchLink(taskId, threadId.trim(), compared))}
            >
              Link existing thread
            </button>
          </div>
        </>
      )}

      {error && <p className="desc error">{error}</p>}
    </div>
  );
}
