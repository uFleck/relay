import { useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { errorText, inTauri, mcpToken, type McpStatus } from "../ipc";
import type { SyncState } from "../types";
import { Spinner } from "./Slack";

/* Settings → Agents & MCP: the loopback server's state, its two endpoints and tokens, and the config
   snippets for Executor (agent endpoint) and OpenCode (analyst endpoint). */

const EXECUTOR_SERVERS = String.raw`C:\Projects\executor-bridges\servers.json`;

type Which = "agent" | "analyst";

/** The dot colour: green when running, red when Rust reported an error, amber while it is still starting. */
export function mcpDotState(status: McpStatus): SyncState {
  if (status.running) return "ok";
  return status.error ? "down" : "retrying";
}

function mcpLabel(status: McpStatus) {
  if (status.running) return `Running on 127.0.0.1:${status.port}`;
  return status.error ? "MCP did not start" : "MCP is starting…";
}

/** The status payload carries no tokens: the window asks Rust for one only when you reveal or copy it. */
const MASK = "••••••••";

async function loadToken(which: Which) {
  const token = await mcpToken(which);
  if (!token) throw new Error(inTauri ? `Relay has no ${which} token yet.` : "Tokens are available in the desktop app.");
  return token;
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

interface Secret {
  which: Which;
  /** The real token while it is revealed. */
  token: string | undefined;
  onToken: (token: string | undefined) => void;
  onError: (message: string | null) => void;
}

/** Fetches the token and copies `format(token)`; if the clipboard refuses, reveals and selects the block so Ctrl+C still works. */
function CopyButton({
  target,
  format = (token) => token,
  which,
  onToken,
  onError,
}: { target: React.RefObject<HTMLElement | null>; format?: (token: string) => string } & Omit<Secret, "token">) {
  const [state, setState] = useState<"idle" | "loading" | "copied" | "manual">("idle");
  useEffect(() => {
    if (state === "idle" || state === "loading") return;
    const t = setTimeout(() => setState("idle"), 2400);
    return () => clearTimeout(t);
  }, [state]);
  async function copy() {
    setState("loading");
    onError(null);
    let token: string;
    try {
      token = await loadToken(which);
    } catch (err) {
      onError(errorText(err));
      setState("idle");
      return;
    }
    try {
      // `navigator.clipboard` is missing outside a secure context, so a throw counts as a refusal too.
      await navigator.clipboard.writeText(format(token));
      setState("copied");
    } catch {
      // The block must hold the real token before it is selected.
      flushSync(() => onToken(token));
      selectContents(target.current);
      setState("manual");
    }
  }
  return (
    <button type="button" className="btn" disabled={state === "loading"} onClick={() => void copy()}>
      {state === "loading" && <Spinner />}
      <span aria-live="polite">{state === "copied" ? "Copied" : state === "manual" ? "Selected · Ctrl+C" : "Copy"}</span>
    </button>
  );
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

function TokenField({ label, ...secret }: { label: string } & Secret) {
  const { which, token, onToken, onError } = secret;
  const ref = useRef<HTMLElement>(null);
  const [revealing, setRevealing] = useState(false);
  async function reveal() {
    setRevealing(true);
    onError(null);
    try {
      onToken(await loadToken(which));
    } catch (err) {
      onError(errorText(err));
    } finally {
      setRevealing(false);
    }
  }
  return (
    <>
      <span className="k">{label}</span>
      <code ref={ref}>{token ?? MASK}</code>
      <span className="mcp-act">
        <button
          type="button"
          className="btn ghost"
          aria-pressed={token !== undefined}
          disabled={revealing}
          onClick={() => (token === undefined ? void reveal() : onToken(undefined))}
        >
          {revealing && <Spinner />}
          {token !== undefined ? "Hide" : "Reveal"}
        </button>
        <CopyButton target={ref} which={which} onToken={onToken} onError={onError} />
      </span>
    </>
  );
}

/** A config snippet. It shows `<which token>` unless that token is revealed; Copy fetches and copies the real one. */
function Snippet({
  title,
  note,
  render,
  ...secret
}: { title: string; note: React.ReactNode; render: (token: string) => string } & Secret) {
  const ref = useRef<HTMLPreElement>(null);
  return (
    <div className="snip">
      <div className="snip-h">
        <b>{title}</b>
        <span className="push" />
        <CopyButton target={ref} format={render} which={secret.which} onToken={secret.onToken} onError={secret.onError} />
      </div>
      <p className="mcp-note">{note}</p>
      <pre ref={ref}>{render(secret.token ?? `<${secret.which} token>`)}</pre>
    </div>
  );
}

export function McpSetup({ status, onRetry }: { status: McpStatus | null; onRetry: () => void }) {
  const [tokens, setTokens] = useState<Partial<Record<Which, string>>>({});
  const [tokenError, setTokenError] = useState<string | null>(null);
  if (status === null) return <p className="empty">MCP setup is available in the desktop app.</p>;
  const secret = (which: Which): Secret => ({
    which,
    token: tokens[which],
    onToken: (token) => setTokens((t) => ({ ...t, [which]: token })),
    onError: setTokenError,
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

      <div className="mcp">
        <Field label="Agent URL" value={status.url} />
        <TokenField label="Agent token" {...secret("agent")} />
        <Field label="Analyst URL" value={status.analystUrl} />
        <TokenField label="Analyst token" {...secret("analyst")} />
      </div>
      {tokenError && <p className="mcp-err">Could not read the token: {tokenError}</p>}
      <Snippet
        title="Executor · agent endpoint"
        note={
          <>
            Merge into <code>{EXECUTOR_SERVERS}</code>, then run <code>node sync.mjs</code> in that folder.
          </>
        }
        render={(token) => executorSnippet(status.url, token)}
        {...secret("agent")}
      />
      <Snippet
        title="OpenCode · analyst endpoint"
        note={
          <>
            Merge into the analyst session's <code>opencode.json</code>. The analyst does not go through Executor.
          </>
        }
        render={(token) => openCodeSnippet(status.analystUrl, token)}
        {...secret("analyst")}
      />
      <p className="mcp-note">Tokens are local to this machine. They only open Relay's loopback server on 127.0.0.1.</p>
    </>
  );
}
