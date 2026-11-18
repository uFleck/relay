import { useEffect, useRef, useState } from "react";
import type { McpStatus } from "../ipc";
import type { SyncState } from "../types";

/* Settings → Agents & MCP: the loopback server's state, its two endpoints and tokens, and the config
   snippets for Executor (agent endpoint) and OpenCode (analyst endpoint). */

const EXECUTOR_SERVERS = String.raw`C:\Projects\executor-bridges\servers.json`;

/** The dot colour: green when running, red when Rust reported an error, amber while it is still starting. */
export function mcpDotState(status: McpStatus): SyncState {
  if (status.running) return "ok";
  return status.error ? "down" : "retrying";
}

function mcpLabel(status: McpStatus) {
  if (status.running) return `Running on 127.0.0.1:${status.port}`;
  return status.error ? "MCP did not start" : "MCP is starting…";
}

const bearer = (token: string) => ({ Authorization: `Bearer ${token}` });

function executorSnippet(url: string, token: string) {
  return JSON.stringify({ mcpServers: { relay: { type: "http", url, headers: bearer(token) } } }, null, 2);
}

function openCodeSnippet(url: string, token: string) {
  return JSON.stringify({ mcp: { "relay-analyst": { type: "remote", url, enabled: true, headers: bearer(token) } } }, null, 2);
}

function selectContents(el: HTMLElement | null) {
  if (!el) return;
  const range = document.createRange();
  range.selectNodeContents(el);
  const sel = window.getSelection();
  sel?.removeAllRanges();
  sel?.addRange(range);
}

/** Copies `text`; if the clipboard refuses, selects the matching block so Ctrl+C still works. */
function CopyButton({ text, target }: { text: string; target: React.RefObject<HTMLElement | null> }) {
  const [state, setState] = useState<"idle" | "copied" | "manual">("idle");
  useEffect(() => {
    if (state === "idle") return;
    const t = setTimeout(() => setState("idle"), 2400);
    return () => clearTimeout(t);
  }, [state]);
  const copy = () =>
    // `navigator.clipboard` is missing outside a secure context, so a throw counts as a refusal too.
    Promise.resolve()
      .then(() => navigator.clipboard.writeText(text))
      .then(
        () => setState("copied"),
        () => {
          selectContents(target.current);
          setState("manual");
        },
      );
  return (
    <button type="button" className="btn" onClick={() => void copy()}>
      <span aria-live="polite">{state === "copied" ? "Copied" : state === "manual" ? "Selected · Ctrl+C" : "Copy"}</span>
    </button>
  );
}

function Field({ label, value, copy }: { label: string; value: string; copy?: boolean }) {
  const ref = useRef<HTMLElement>(null);
  return (
    <>
      <span className="k">{label}</span>
      <code ref={ref}>{value}</code>
      {copy ? <CopyButton text={value} target={ref} /> : <span />}
    </>
  );
}

function Snippet({ title, note, text }: { title: string; note: React.ReactNode; text: string }) {
  const ref = useRef<HTMLPreElement>(null);
  return (
    <div className="snip">
      <div className="snip-h">
        <b>{title}</b>
        <span className="push" />
        <CopyButton text={text} target={ref} />
      </div>
      <p className="mcp-note">{note}</p>
      <pre ref={ref}>{text}</pre>
    </div>
  );
}

export function McpSetup({ status, onRetry }: { status: McpStatus | null; onRetry: () => void }) {
  if (status === null) return <p className="empty">MCP setup is available in the desktop app.</p>;
  const { agentToken, analystToken } = status;
  return (
    <>
      <div className="setrow" role="status">
        <i className={`dot sync-${mcpDotState(status)}`} />
        <span>{mcpLabel(status)}</span>
        <span className="push" />
        {status.error && (
          <button type="button" className="btn" onClick={onRetry}>
            Retry
          </button>
        )}
      </div>
      {status.error && <p className="mcp-err">{status.error}</p>}

      {agentToken && analystToken && (
        <>
          <div className="mcp">
            <Field label="Agent URL" value={status.url} />
            <Field label="Agent token" value={agentToken} copy />
            <Field label="Analyst URL" value={status.analystUrl} />
            <Field label="Analyst token" value={analystToken} copy />
          </div>
          <Snippet
            title="Executor · agent endpoint"
            note={
              <>
                Merge into <code>{EXECUTOR_SERVERS}</code>, then run <code>node sync.mjs</code> in that folder.
              </>
            }
            text={executorSnippet(status.url, agentToken)}
          />
          <Snippet
            title="OpenCode · analyst endpoint"
            note={
              <>
                Merge into the analyst session's <code>opencode.json</code>. The analyst does not go through Executor.
              </>
            }
            text={openCodeSnippet(status.analystUrl, analystToken)}
          />
        </>
      )}
      <p className="mcp-note">Tokens are local to this machine. They only open Relay's loopback server on 127.0.0.1.</p>
    </>
  );
}
