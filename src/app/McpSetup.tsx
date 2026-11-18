import { useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
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

/** PLAN 6: the window shows a token's last 4 characters until you reveal it; Copy always copies the real one. */
const mask = (token: string) => `••••••••${token.slice(-4)}`;

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

/** Copies `text`; if the clipboard refuses, reveals and selects the matching block so Ctrl+C still works. */
function CopyButton({ text, target, reveal }: { text: string; target: React.RefObject<HTMLElement | null>; reveal: () => void }) {
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
          // The block must hold the real token before it is selected.
          flushSync(reveal);
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

interface Secret {
  shown: boolean;
  onShown: (shown: boolean) => void;
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <>
      <span className="k">{label}</span>
      <code>{value}</code>
      <span />
    </>
  );
}

function TokenField({ label, token, shown, onShown }: { label: string; token: string } & Secret) {
  const ref = useRef<HTMLElement>(null);
  return (
    <>
      <span className="k">{label}</span>
      <code ref={ref}>{shown ? token : mask(token)}</code>
      <span className="mcp-act">
        <button type="button" className="btn ghost" aria-pressed={shown} onClick={() => onShown(!shown)}>
          {shown ? "Hide" : "Reveal"}
        </button>
        <CopyButton text={token} target={ref} reveal={() => onShown(true)} />
      </span>
    </>
  );
}

/** A config snippet. It shows the masked token unless that token is revealed; Copy copies the real one. */
function Snippet({ title, note, text, masked, shown, onShown }: { title: string; note: React.ReactNode; text: string; masked: string } & Secret) {
  const ref = useRef<HTMLPreElement>(null);
  return (
    <div className="snip">
      <div className="snip-h">
        <b>{title}</b>
        <span className="push" />
        <CopyButton text={text} target={ref} reveal={() => onShown(true)} />
      </div>
      <p className="mcp-note">{note}</p>
      <pre ref={ref}>{shown ? text : masked}</pre>
    </div>
  );
}

export function McpSetup({ status, onRetry }: { status: McpStatus | null; onRetry: () => void }) {
  const [shown, setShown] = useState({ agent: false, analyst: false });
  if (status === null) return <p className="empty">MCP setup is available in the desktop app.</p>;
  const { agentToken, analystToken } = status;
  const secret = (which: "agent" | "analyst"): Secret => ({
    shown: shown[which],
    onShown: (on) => setShown((s) => ({ ...s, [which]: on })),
  });
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
            <TokenField label="Agent token" token={agentToken} {...secret("agent")} />
            <Field label="Analyst URL" value={status.analystUrl} />
            <TokenField label="Analyst token" token={analystToken} {...secret("analyst")} />
          </div>
          <Snippet
            title="Executor · agent endpoint"
            note={
              <>
                Merge into <code>{EXECUTOR_SERVERS}</code>, then run <code>node sync.mjs</code> in that folder.
              </>
            }
            text={executorSnippet(status.url, agentToken)}
            masked={executorSnippet(status.url, mask(agentToken))}
            {...secret("agent")}
          />
          <Snippet
            title="OpenCode · analyst endpoint"
            note={
              <>
                Merge into the analyst session's <code>opencode.json</code>. The analyst does not go through Executor.
              </>
            }
            text={openCodeSnippet(status.analystUrl, analystToken)}
            masked={openCodeSnippet(status.analystUrl, mask(analystToken))}
            {...secret("analyst")}
          />
        </>
      )}
      <p className="mcp-note">Tokens are local to this machine. They only open Relay's loopback server on 127.0.0.1.</p>
    </>
  );
}
