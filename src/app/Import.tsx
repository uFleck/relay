import { useEffect, useState, type FormEvent } from "react";
import {
  clickupClearToken,
  clickupScope,
  clickupSetScope,
  clickupSetToken,
  clickupSpaces,
  clickupStatus,
  clickupSyncNow,
  clickupWorkspaces,
  errorText,
  inTauri,
  type ClickupSyncResult,
  type ImportCount,
} from "../ipc";
import type { Project } from "../types";
import { Spinner, useLive } from "./Slack";
import { Select } from "../ui/controls";

/* Legacy ClickUp scope controls and the shared repo-import result renderer. Create-only Note
   Import lives in the v2 Settings Import section; errors stay inline. */

/* ── Settings → ClickUp ── */

export function ClickupSettings({ projects }: { projects: Project[] }) {
  const { data: status, error: loadError, mutate } = useLive(clickupStatus, []);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState<"save" | "clear" | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function save(e: FormEvent) {
    e.preventDefault();
    if (!inTauri) {
      setError("ClickUp runs in the desktop app.");
      return;
    }
    setBusy("save");
    setError(null);
    try {
      const next = await clickupSetToken(token.trim());
      mutate(() => next);
      setToken("");
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  }

  async function clear() {
    setBusy("clear");
    setError(null);
    try {
      const next = await clickupClearToken();
      mutate(() => next);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  }

  if (!status) return loadError ? <p className="mcp-err">{loadError}</p> : <p className="empty">Reading ClickUp status…</p>;
  return (
    <>
      <div className="setrow" role="status">
        <i className={`dot sync-${status.configured ? "ok" : "off"}`} />
        <span className={status.configured ? "ok-fg" : undefined}>{status.configured ? "Connected" : "No token"}</span>
        <span className="push" />
        {status.configured && (
          <button type="button" className="btn bad" disabled={busy !== null} onClick={() => void clear()}>
            {busy === "clear" ? "Clearing…" : "Clear"}
          </button>
        )}
      </div>
      {error && (
        <p className="mcp-err" role="alert">
          {error}
        </p>
      )}
      <form className="sform" onSubmit={(e) => void save(e)}>
        <label htmlFor="cu-token">{status.configured ? "Replace token" : "Token"}</label>
        <input
          id="cu-token"
          className="inp mono"
          type="password"
          autoComplete="off"
          spellCheck={false}
          placeholder="pk_…"
          value={token}
          onChange={(e) => setToken(e.target.value)}
        />
        <span />
        <div className="sform-f">
          <button type="submit" className="btn pri" disabled={busy !== null || !token.trim()}>
            {busy === "save" && <Spinner />}
            {busy === "save" ? "Checking token…" : "Save"}
          </button>
        </div>
      </form>
      <p className="mcp-note">
        A personal token reaches every workspace your ClickUp account can see. Save checks it with one real ClickUp call; it
        stays in Windows Credential Manager. Relay only imports from ClickUp and never writes back.
      </p>
      {status.configured && inTauri && <TrackAssigned projects={projects} />}
    </>
  );
}

/* ── Settings → ClickUp → cards assigned to me ── */

function TrackAssigned({ projects }: { projects: Project[] }) {
  const scope = useLive(clickupScope, []);
  const workspaces = useLive(clickupWorkspaces, []);
  const [workspaceId, setWorkspaceId] = useState<string | null>(null);
  const [spaceId, setSpaceId] = useState<string | null>(null);
  const [projectId, setProjectId] = useState<string | null>(null);
  const [spaces, setSpaces] = useState<{ id: string; name: string }[]>([]);
  const [busy, setBusy] = useState<"save" | "sync" | null>(null);
  const [result, setResult] = useState<ClickupSyncResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const saved = scope.data?.scope ?? null;
  const workspace = workspaceId ?? saved?.workspaceId ?? workspaces.data?.[0]?.id ?? "";
  const space = spaceId ?? (workspaceId === null ? saved?.spaceId : null) ?? "";
  const project = projectId ?? saved?.projectId ?? projects.find((p) => p.name === "Inbox")?.id ?? projects[0]?.id ?? "";
  const shown = result ?? scope.data?.last ?? null;

  useEffect(() => {
    if (!workspace) return;
    let live = true;
    clickupSpaces(workspace)
      .then((next) => live && setSpaces(next))
      .catch((err) => live && setError(errorText(err)));
    return () => {
      live = false;
    };
  }, [workspace]);

  async function run(kind: "save" | "sync") {
    setBusy(kind);
    setError(null);
    try {
      if (kind === "save") await clickupSetScope({ workspaceId: workspace, spaceId: space || null, projectId: project });
      setResult(await clickupSyncNow());
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  }

  return (
    <>
      <h4 className="sub">Track cards assigned to me</h4>
      <div className="sform">
        <label htmlFor="cu-workspace">Workspace</label>
        <Select
          id="cu-workspace"
          value={workspace}
          disabled={busy !== null}
          options={(workspaces.data ?? []).map((w) => ({ value: w.id, label: w.name }))}
          onChange={(next) => {
            setWorkspaceId(next);
            setSpaceId(null);
            setSpaces([]);
          }}
        />
        <label htmlFor="cu-space">Space</label>
        <Select
          id="cu-space"
          value={space}
          disabled={busy !== null}
          options={[{ value: "", label: "All spaces I can see" }, ...spaces.map((s) => ({ value: s.id, label: s.name }))]}
          onChange={setSpaceId}
        />
        <label htmlFor="cu-project">Relay project</label>
        <Select
          id="cu-project"
          value={project}
          disabled={busy !== null}
          options={projects.map((p) => ({ value: p.id, label: p.name }))}
          onChange={setProjectId}
        />
        <span />
        <div className="sform-f">
          <button type="button" className="btn pri" disabled={busy !== null || !workspace || !project} onClick={() => void run("save")}>
            {busy === "save" && <Spinner />}
            {busy === "save" ? "Syncing…" : "Save and sync"}
          </button>
          {saved && (
            <button type="button" className="btn" disabled={busy !== null} onClick={() => void run("sync")}>
              {busy === "sync" && <Spinner />}
              {busy === "sync" ? "Syncing…" : "Sync now"}
            </button>
          )}
        </div>
      </div>
      {(error ?? scope.data?.error) && (
        <p className="mcp-err" role="alert">
          {error ?? scope.data?.error}
        </p>
      )}
      {shown && (
        <p className="mcp-note" role="status">
          {shown.created} new, {shown.updated} updated, {shown.archived} archived
          {shown.truncated && ". The list was cut off, so nothing was archived this time"}
        </p>
      )}
      <p className="mcp-note">
        Relay syncs every 10 minutes and brings in only open cards assigned to you. A card that is reassigned or closed in
        ClickUp is archived here, never deleted.
      </p>
    </>
  );
}

/* ── Projects → Import ── */

type Run = { kind: "busy" } | { kind: "done"; count: ImportCount } | { kind: "error"; text: string } | null;

export function RunResult({ run }: { run: Run }) {
  if (run?.kind === "done") {
    return (
      <span className="meta" role="status">
        Imported {run.count.imported}, skipped {run.count.skipped}
      </span>
    );
  }
  if (run?.kind === "error") {
    return (
      <span className="imp-err" role="alert">
        {run.text}
      </span>
    );
  }
  return null;
}

/** Runs one import and keeps its outcome; refuses overlapping runs. */
export function useRun() {
  const [run, setRun] = useState<Run>(null);
  async function start(go: () => Promise<ImportCount>) {
    if (run?.kind === "busy") return;
    setRun({ kind: "busy" });
    try {
      setRun({ kind: "done", count: await go() });
    } catch (err) {
      setRun({ kind: "error", text: errorText(err) });
    }
  }
  return { run, busy: run?.kind === "busy", start };
}
