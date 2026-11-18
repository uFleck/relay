import { useState, type FormEvent } from "react";
import { clickupClearToken, clickupImport, clickupSetToken, clickupStatus, errorText, githubImportIssues, inTauri, type ImportCount } from "../ipc";
import type { Project } from "../types";
import { Spinner, useLive } from "./Slack";

/* ClickUp and GitHub Issues import: the Settings section for the ClickUp token and the per-project
   panel on Projects. Imports are one-shot and link-only; errors stay inline. */

/* ── Settings → ClickUp ── */

export function ClickupSettings() {
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
    </>
  );
}

/* ── Projects → Import ── */

type Run = { kind: "busy" } | { kind: "done"; count: ImportCount } | { kind: "error"; text: string } | null;

function RunResult({ run }: { run: Run }) {
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
function useRun() {
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

export function ProjectImport({ project }: { project: Project }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <div className="repo">
        <span className="meta">Import tasks from ClickUp or GitHub Issues</span>
        <span className="push" />
        <button type="button" className="btn" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
          {open ? "Hide" : "Import…"}
        </button>
      </div>
      {open && <ImportPanel project={project} />}
    </>
  );
}

function ImportPanel({ project }: { project: Project }) {
  const clickup = useLive(clickupStatus, []);
  const [listId, setListId] = useState("");
  const [includeClosed, setIncludeClosed] = useState(false);
  const [owner, setOwner] = useState("");
  const [repo, setRepo] = useState("");
  const cu = useRun();
  const gh = useRun();

  if (!inTauri) {
    return (
      <div className="imp">
        <p className="imp-note">Import runs in the desktop app. The browser preview has no ClickUp or GitHub connection.</p>
      </div>
    );
  }

  const noToken = clickup.data?.configured === false;
  const importList = (e: FormEvent) => {
    e.preventDefault();
    void cu.start(() => clickupImport({ projectId: project.id, listId: listId.trim(), includeClosed }));
  };
  const importIssues = (e: FormEvent) => {
    e.preventDefault();
    void gh.start(() => githubImportIssues({ projectId: project.id, owner: owner.trim(), repo: repo.trim() }));
  };

  return (
    <div className="imp">
      <p className="imp-note">
        Each imported task keeps a link to its source. Running an import again skips what already exists. Relay does not
        mirror later changes in either direction.
      </p>

      <form className="imp-row" onSubmit={importList}>
        <b>ClickUp</b>
        <input
          className="inp"
          aria-label="ClickUp list id"
          placeholder="List id"
          spellCheck={false}
          value={listId}
          disabled={cu.busy}
          onChange={(e) => setListId(e.target.value)}
        />
        <label className="imp-chk">
          <input type="checkbox" checked={includeClosed} disabled={cu.busy} onChange={(e) => setIncludeClosed(e.target.checked)} />
          Include closed
        </label>
        <button type="submit" className="btn pri" disabled={cu.busy || noToken || !listId.trim()}>
          {cu.busy && <Spinner />}
          {cu.busy ? "Importing…" : "Import list"}
        </button>
        <RunResult run={cu.run} />
      </form>
      {noToken && <p className="imp-note">Add a ClickUp token in Settings to import lists.</p>}
      {clickup.error && <p className="imp-err">{clickup.error}</p>}

      <form className="imp-row" onSubmit={importIssues}>
        <b>GitHub</b>
        <input
          className="inp"
          aria-label="GitHub owner"
          placeholder="owner"
          spellCheck={false}
          value={owner}
          disabled={gh.busy}
          onChange={(e) => setOwner(e.target.value)}
        />
        <input
          className="inp"
          aria-label="GitHub repository"
          placeholder="repo"
          spellCheck={false}
          value={repo}
          disabled={gh.busy}
          onChange={(e) => setRepo(e.target.value)}
        />
        <button type="submit" className="btn pri" disabled={gh.busy || !owner.trim() || !repo.trim()}>
          {gh.busy && <Spinner />}
          {gh.busy ? "Importing…" : "Import open issues"}
        </button>
        <RunResult run={gh.run} />
      </form>
    </div>
  );
}
