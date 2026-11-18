import { useEffect, useState, type FormEvent } from "react";
import { Icon } from "../components/Icon";
import { prMeta, prTitle, type Checks, type Review } from "../github";
import {
  errorText,
  githubClearToken,
  githubSetToken,
  githubStatus,
  inTauri,
  onEvent,
  projectGitState,
  projectSetRepo,
  reposImport,
  reposScan,
  syncPrsNow,
  type GitState,
  type GithubStatus,
} from "../ipc";
import { plural } from "../model";
import type { Project, RepoCandidate, Task } from "../types";
import { RunResult, useRun } from "./Import";
import { Spinner, useLive } from "./Slack";

/* GitHub and git context: the Settings section, each project's checkout on Projects and the task
   sheet's Context block. Errors stay inline; nothing here opens a native dialog. */

/* ── Settings → GitHub ── */

function sourceLine(s: GithubStatus): string {
  if (s.source === "token") return "Using your stored token";
  if (s.source === "gh") return s.login ? `Using your gh session (${s.login})` : "Using your gh session";
  return "No gh session or token (anonymous)";
}

export function GithubSettings() {
  const { data: status, error: loadError, mutate } = useLive(githubStatus, []);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState<"save" | "clear" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [synced, setSynced] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => onEvent("github://status", (next) => mutate(() => next)), [mutate]);

  async function save(e: FormEvent) {
    e.preventDefault();
    if (!inTauri) {
      setError("GitHub runs in the desktop app.");
      return;
    }
    setBusy("save");
    setError(null);
    try {
      const next = await githubSetToken(token.trim());
      mutate(() => next);
      if (next.error) setError(next.error);
      else setToken("");
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
      const next = await githubClearToken();
      mutate(() => next);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  }

  async function sync() {
    setSyncing(true);
    setSynced(null);
    try {
      const n = await syncPrsNow();
      setSynced({ ok: true, text: `Updated ${plural(n, "pull request")}` });
    } catch (err) {
      setSynced({ ok: false, text: errorText(err) });
    } finally {
      setSyncing(false);
    }
  }

  if (!status) return loadError ? <p className="mcp-err">{loadError}</p> : <p className="empty">Reading GitHub status…</p>;
  const shown = error ?? status.error;
  return (
    <>
      <div className="setrow" role="status">
        <i className={`dot sync-${status.error ? "down" : status.source !== "none" ? "ok" : "off"}`} />
        <span className={status.source !== "none" && !status.error ? "ok-fg" : undefined}>
          {sourceLine(status)}
        </span>
        <span className="push" />
        {status.configured && (
          <button type="button" className="btn bad" disabled={busy !== null} onClick={() => void clear()}>
            {busy === "clear" ? "Clearing…" : "Clear"}
          </button>
        )}
      </div>
      {shown && (
        <p className="mcp-err" role="alert">
          {shown}
        </p>
      )}
      <form className="sform" onSubmit={(e) => void save(e)}>
        <label htmlFor="gh-token">{status.configured ? "Replace token" : "Token override (optional)"}</label>
        <input
          id="gh-token"
          className="inp mono"
          type="password"
          autoComplete="off"
          spellCheck={false}
          placeholder="ghp_…"
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
        Relay uses your active gh session when there is one (run <code>gh auth login</code> once). A token here is an
        optional override that wins over gh. Without either, requests are anonymous and share GitHub's low rate limit.
        Save checks the token with one real GitHub call; it stays in Windows Credential Manager and Relay never shows
        it again.
      </p>
      <div className="setrow">
        <button type="button" className="btn" disabled={syncing} onClick={() => void sync()}>
          {syncing && <Spinner />}
          {syncing ? "Syncing…" : "Sync pull requests now"}
        </button>
        {synced?.ok && <span className="meta">{synced.text}</span>}
      </div>
      {synced && !synced.ok && (
        <p className="mcp-err" role="alert">
          {synced.text}
        </p>
      )}
      <RepoFolder />
    </>
  );
}

/* ── Settings → GitHub → Repositories folders ── */

const FOLDERS_KEY = "relay.reposFolders";
const OLD_FOLDER_KEY = "relay.reposFolder";

function rememberedFolders(): string[] {
  try {
    const saved = localStorage.getItem(FOLDERS_KEY);
    if (saved) {
      const parsed: unknown = JSON.parse(saved);
      if (Array.isArray(parsed)) return parsed.filter((f): f is string => typeof f === "string");
    }
    const old = localStorage.getItem(OLD_FOLDER_KEY);
    return old ? [old] : [];
  } catch {
    return [];
  }
}

function rememberFolders(folders: string[]) {
  try {
    localStorage.setItem(FOLDERS_KEY, JSON.stringify(folders));
    localStorage.removeItem(OLD_FOLDER_KEY);
  } catch {
    /* a convenience only */
  }
}

const trimSlash = (path: string) => path.replace(/[\\/]+$/, "").toLowerCase();
const samePath = (a: string, b: string) => trimSlash(a) === trimSlash(b);

function RepoFolder() {
  const [folders, setFolders] = useState<string[]>(rememberedFolders);
  const [found, setFound] = useState<RepoCandidate[] | null>(null);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const imp = useRun();

  async function scan(paths: string[]) {
    setError(null);
    setFound(null);
    if (paths.length === 0) return;
    setScanning(true);
    try {
      setFound(await reposScan(paths));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setScanning(false);
    }
  }

  function update(next: string[]) {
    setFolders(next);
    rememberFolders(next);
    void scan(next);
  }

  async function add() {
    if (!inTauri) {
      setError("Choosing a folder works in the desktop app.");
      return;
    }
    setError(null);
    let picked: string | null;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      picked = await open({ directory: true, multiple: false, title: "Folder that holds your clones", defaultPath: folders[0] });
    } catch (err) {
      setError(errorText(err));
      return;
    }
    if (!picked) return;
    update(folders.some((f) => samePath(f, picked)) ? folders : [...folders, picked]);
  }

  const fresh = found?.filter((c) => !c.projectId).length ?? 0;
  const importAll = () => {
    if (folders.length === 0) return;
    void imp.start(async () => {
      const count = await reposImport(folders);
      await scan(folders);
      return count;
    });
  };

  return (
    <>
      <h3 className="sub">Repositories folders</h3>
      {folders.length === 0 && <p className="empty">No folder chosen.</p>}
      {folders.length > 0 && (
        <ul className="repo-list">
          {folders.map((f) => (
            <li key={f} className="repo">
              <span className="mono ellip" title={f}>
                {f}
              </span>
              <span className="push" />
              <button
                type="button"
                className="btn"
                disabled={scanning || imp.busy}
                onClick={() => update(folders.filter((other) => other !== f))}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="setrow">
        <button type="button" className="btn" disabled={scanning || imp.busy} onClick={() => void add()}>
          {scanning && <Spinner />}
          Add folder…
        </button>
        {folders.length > 0 && !found && !scanning && (
          <button type="button" className="btn" onClick={() => void scan(folders)}>
            Scan
          </button>
        )}
      </div>
      {error && (
        <p className="mcp-err" role="alert">
          {error}
        </p>
      )}
      {found && found.length === 0 && <p className="empty">No clones with a GitHub origin in these folders.</p>}
      {found && found.length > 0 && (
        <>
          <ul className="repo-list">
            {found.map((c) => (
              <li key={c.path} className="repo">
                <b>
                  {c.repo.owner}/{c.repo.name}
                </b>
                <span className="mono ellip meta" title={c.path}>
                  {c.path}
                </span>
                <span className="push" />
                {c.projectId && <span className="meta">project</span>}
              </li>
            ))}
          </ul>
          <div className="setrow">
            <button type="button" className="btn pri" disabled={imp.busy || fresh === 0} onClick={importAll}>
              {imp.busy && <Spinner />}
              {imp.busy ? "Importing…" : `Import ${plural(fresh, "repo")}`}
            </button>
            <RunResult run={imp.run} />
          </div>
        </>
      )}
      <p className="mcp-note">
        Pick the folder that holds your clones, such as C:\umbler; it need not be a repository itself. Relay looks at the
        folder and its direct subfolders. Each clone becomes a project. Pull requests you authored or were asked to review
        in these repositories become tasks automatically every 3 minutes, using your gh session or token.
      </p>
    </>
  );
}

/* ── git state ── */

export function GitLine({ state }: { state: GitState }) {
  return (
    <span className="gitl">
      <Icon name="branch" small />
      <b className="mono ellip">{state.branch ?? "detached"}</b>
      {state.head && <span className="mono meta">{state.head.slice(0, 7)}</span>}
      {state.dirty && <i className="gdirty" role="img" aria-label="Uncommitted changes" title="Uncommitted changes" />}
      {state.headSubject && (
        <span className="meta ellip" title={state.headSubject}>
          {state.headSubject}
        </span>
      )}
    </span>
  );
}

function RefreshButton({ onClick, busy }: { onClick: () => void; busy?: boolean }) {
  return (
    <button type="button" className="ib" title="Refresh git state" aria-label="Refresh git state" disabled={busy} onClick={onClick}>
      <Icon name="refresh" small />
    </button>
  );
}

/* ── Projects → repository ── */

export function ProjectRepo({ project }: { project: Project }) {
  // `undefined`: the snapshot did not say. The git state then tells whether a checkout is linked.
  const [path, setPath] = useState(project.repoPath);
  const git = useLive(() => projectGitState(project.id), [project.id], { onChange: false });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => setPath(project.repoPath), [project.repoPath]);

  const linked = path ? true : path === null ? false : git.data != null || git.error != null;

  async function setRepo(next: string | null) {
    setBusy(true);
    setError(null);
    try {
      await projectSetRepo(project.id, next);
      setPath(next);
      await git.reload();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  }

  async function choose() {
    if (!inTauri) {
      setError("Choosing a folder works in the desktop app.");
      return;
    }
    setError(null);
    let picked: string | null;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      picked = await open({ directory: true, multiple: false, title: `Repository for ${project.name}`, defaultPath: path ?? undefined });
    } catch (err) {
      setError(errorText(err));
      return;
    }
    if (picked) await setRepo(picked);
  }

  const shown = error ?? git.error;
  return (
    <>
      <div className="repo">
        <span className={`mono ellip${linked ? "" : " meta"}`} title={path ?? undefined}>
          {path ?? (linked ? "Repository linked" : "No repo")}
        </span>
        {git.data && <GitLine state={git.data} />}
        <span className="push" />
        {linked && <RefreshButton busy={busy} onClick={() => void git.reload()} />}
        <button type="button" className="btn" disabled={busy} onClick={() => void choose()}>
          {busy && <Spinner />}
          Choose folder
        </button>
        {linked && (
          <button type="button" className="btn" disabled={busy} onClick={() => void setRepo(null)}>
            Clear
          </button>
        )}
      </div>
      {shown && (
        <p className="mcp-err" role="alert">
          {shown}
        </p>
      )}
    </>
  );
}

/* ── Task sheet → Context ── */

const STATE_TONE: Record<string, string> = { open: "ok", draft: "mut", merged: "acc", closed: "bad" };
const CHECK_TONE: Record<Checks, string> = { none: "mut", pending: "warn", passing: "ok", failing: "bad" };
const REVIEW_TONE: Record<Review, string> = { none: "mut", pending: "warn", approved: "ok", changes_requested: "bad" };

/** The task's project checkout and its PRs. Renders nothing when there is neither. Key it by project. */
export function TaskContext({ task, onOpen }: { task: Task; onOpen: (url: string) => void }) {
  const git = useLive(() => projectGitState(task.projectId), [task.projectId], { onChange: false });
  const prs = task.links.filter((l) => l.kind === "pr");
  if (!git.data && !git.error && prs.length === 0) return null;

  return (
    <section className="sec">
      <h3>Context</h3>
      {(git.data || git.error) && (
        <div className="ctx-git">
          {git.data ? <GitLine state={git.data} /> : <span className="err-fg ellip">{git.error}</span>}
          <span className="push" />
          <RefreshButton onClick={() => void git.reload()} />
        </div>
      )}
      {prs.map((l) => {
        const meta = prMeta(l.metadataJson);
        const title = prTitle(l);
        return (
          <div className="prrow" key={l.url}>
            <Icon name="pr" small className={`pr-${l.state}`} />
            <span className="ellip" title={title === l.url ? l.url : `${title}\n${l.url}`}>
              {title}
            </span>
            <button type="button" className="ib" title="Open on GitHub" aria-label="Open on GitHub" onClick={() => onOpen(l.url)}>
              <Icon name="ext" small />
            </button>
            <span className="prchips">
              <span className={`tag ${STATE_TONE[l.state] ?? "mut"}`}>{l.state || "unknown"}</span>
              <span className={`tag ${CHECK_TONE[meta.checks]}`} title="Checks">
                checks {meta.checks}
              </span>
              <span className={`tag ${REVIEW_TONE[meta.review]}`} title="Review">
                review {meta.review.replace("_", " ")}
              </span>
            </span>
          </div>
        );
      })}
    </section>
  );
}
