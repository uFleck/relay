import { useCallback, useEffect, useRef, useState } from "react";
import {
  errorText,
  onEvent,
  openT3,
  v2T3CheckAgain,
  v2T3Close,
  v2T3Progress,
  v2T3Refresh,
  v2T3Resend,
  v2T3Send,
  type V2T3Progress,
} from "../ipc";
import type { V2Task } from "../ipc";

/* The observed half of a dispatched Task. Every action is explicit: Relay never closes a Task
   because a PR merged or T3 settled a thread, and a cancellation confirms before committing. */

interface Props {
  task: V2Task;
  onChanged?: () => void;
}

export function T3ProgressPanel({ task, onChanged }: Props) {
  const [view, setView] = useState<V2T3Progress | null>(null);
  const [draft, setDraft] = useState("");
  const [resendConfirmed, setResendConfirmed] = useState(false);
  const [confirming, setConfirming] = useState<"done" | "cancelled" | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refreshedFor = useRef<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setView(await v2T3Progress(task.id));
    } catch (cause) {
      setError(errorText(cause));
    }
  }, [task.id]);

  useEffect(() => {
    void refresh();
    const offProgress = onEvent("relay://t3-progress", (id) => {
      if (!id || id === task.id) void refresh();
    });
    const offChange = onEvent("relay://v2-change", () => void refresh());
    return () => {
      offProgress();
      offChange();
    };
  }, [task.id, refresh]);

  // An open Returned or Failed sheet refreshes thread metadata once per status, never in a loop.
  useEffect(() => {
    const open = task.status === "returned" || task.status === "failed";
    const token = open ? task.id : null;
    if (token && refreshedFor.current !== token) {
      refreshedFor.current = token;
      void v2T3Refresh(task.id).then(refresh).catch(() => undefined);
    }
    if (!open) refreshedFor.current = null;
  }, [task.id, task.status, refresh]);

  async function run(action: () => Promise<void>, success?: string) {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await action();
      if (success) setNotice(success);
      onChanged?.();
      await refresh();
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  }

  async function send(kind: "followup" | "retry") {
    const result = await v2T3Send(task.id, kind === "retry" ? "" : draft, kind);
    if (result.outcome === "sent") {
      setDraft("");
      setNotice(result.message);
      return;
    }
    if (result.outcome === "draft_unsent") {
      // Keep the draft: T3 was already active and nothing was sent.
      setNotice(result.message);
      return;
    }
    setNotice(result.message);
  }

  async function close(status: "done" | "cancelled") {
    setConfirming(null);
    await run(() => v2T3Close(task.id, status), "Relay stopped tracking this Task.");
  }

  if (!view) return null;
  const active = task.status === "dispatched" || task.status === "needs_answer";
  const sendState = view.activeSend?.state;
  const threadLink = view.threadUrl ? (
    <a className="lnk" href={view.threadUrl} target="_blank" rel="noreferrer">
      Open in T3
    </a>
  ) : null;

  return (
    <section className="t3-progress">
      <h3>T3 progress</h3>
      <p className="desc meta">
        {view.taskStatus.replace(/_/g, " ")}
        {view.status ? ` · run ${view.status} · ${view.runCount} run(s)` : ""}
      </p>

      {view.staleReason && (
        <p className="desc" role="status">
          {view.staleReason} Open T3 to check the thread, or review the connection in Settings.
        </p>
      )}

      {task.status === "needs_answer" && (
        <div className="t3-needs-answer">
          <p className="desc">T3 is waiting for your answer. Answer it in the same thread.</p>
          {threadLink}
        </div>
      )}

      {view.pendingRequestCount > 0 && task.status !== "needs_answer" && (
        <p className="desc">{view.pendingRequestCount} pending request(s) in T3.</p>
      )}

      {view.branch && <p className="desc meta">branch {view.branch}</p>}
      {view.worktreePath && <p className="desc meta">worktree {view.worktreePath}</p>}
      {view.reply && (
        <div className="t3-reply">
          <p className="desc">Reply</p>
          <pre className="t3-reply-text">{view.reply}</pre>
        </div>
      )}
      {view.error && <p className="desc error">{view.error}</p>}

      {view.pullRequests.length > 0 && (
        <div className="t3-prs">
          <p className="desc">Linked PRs</p>
          <ul>
            {view.pullRequests.map((pull) => (
              <li key={pull.url}>
                <a className="lnk" href={pull.url} target="_blank" rel="noreferrer">
                  {pull.url}
                </a>{" "}
                <span className="meta mono">{pull.state ?? "state unknown"}</span>
              </li>
            ))}
          </ul>
          <p className="desc meta">A PR merge never closes this Task.</p>
        </div>
      )}

      {sendState === "needs_confirmation" && (
        <div className="t3-confirm-send">
          <p className="desc">
            The send response was lost. Check the thread in T3, then confirm whether the message
            arrived.
          </p>
          {threadLink}
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => void run(() => v2T3CheckAgain(task.id).then(() => undefined))}
          >
            Check again
          </button>
          <label className="desc">
            <input
              type="checkbox"
              checked={resendConfirmed}
              onChange={(event) => setResendConfirmed(event.target.checked)}
            />{" "}
            I confirmed in T3 that the first message was absent.
          </label>
          <button
            type="button"
            className="btn"
            disabled={busy || !resendConfirmed}
            onClick={() => void run(() => v2T3Resend(task.id, true).then(() => undefined))}
          >
            Resend to this thread
          </button>
        </div>
      )}

      {sendState === "failed" && (
        <div className="t3-confirm-send">
          <p className="desc">{view.activeSend?.failureReason ?? "T3 rejected the message."}</p>
          <p className="desc meta">The draft is kept: “{view.activeSend?.message}”</p>
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => void run(() => v2T3Resend(task.id, true).then(() => undefined))}
          >
            Retry the retained message
          </button>
        </div>
      )}

      {view.canFollowUp && (
        <div className="t3-followup">
          <label className="desc" htmlFor={`t3-followup-${task.id}`}>
            Follow up on the same thread
          </label>
          <textarea
            id={`t3-followup-${task.id}`}
            className="input t3-followup-input"
            rows={4}
            value={draft}
            placeholder="Write the message verbatim…"
            onChange={(event) => setDraft(event.target.value)}
          />
          <button
            type="button"
            className="btn"
            disabled={busy || draft.length === 0}
            onClick={() => void run(() => send("followup"))}
          >
            Follow up
          </button>
        </div>
      )}

      {view.canRetryRun && (
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => void run(() => send("retry"))}
        >
          Retry the T3 run on this thread
        </button>
      )}

      {active && threadLink}

      {view.canClose && (
        <div className="t3-close">
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => void run(() => v2T3Close(task.id, "done"), "Closed as done.")}
          >
            Close as done
          </button>
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => (active ? setConfirming("cancelled") : void close("cancelled"))}
          >
            Close as cancelled
          </button>
        </div>
      )}

      {confirming === "cancelled" && (
        <div className="t3-dialog" role="dialog" aria-modal="true" aria-label="Cancel this Task">
          <p className="desc">
            Relay will stop tracking this Task and will not change T3. Its run may still be active;
            use the thread link to stop it in T3.
          </p>
          {threadLink}
          <div className="t3-dialog-actions">
            <button type="button" className="btn" onClick={() => void close("cancelled")}>
              Stop tracking and cancel
            </button>
            <button type="button" className="btn" onClick={() => setConfirming(null)}>
              Keep the Task
            </button>
          </div>
        </div>
      )}

      {view.staleReason && (
        <button type="button" className="btn" disabled={busy} onClick={() => void openT3()}>
          Open T3
        </button>
      )}
      {notice && <p className="desc" role="status">{notice}</p>}
      {error && <p className="desc error">{error}</p>}
    </section>
  );
}
