import { useCallback, useEffect, useRef, useState } from "react";
import { Icon } from "../components/Icon";
import {
  openT3,
  v2ImportNow,
  v2WorkspaceSnapshot,
  type ImportRunResult,
  type V2Project,
  type V2Settings,
} from "../ipc";
import { setKeybinds, useKeybinds } from "../keybinds";
import type { ProviderKey } from "../route";
import {
  checkT3Mapping,
  clearClickupToken,
  clearGithubToken,
  clearOpencodeKey,
  clickupSpaces,
  clickupWorkspaces,
  errorText,
  getProviderStatus,
  onProviderStatus,
  onSettingsChange,
  refreshProviders,
  registerV2Repo,
  saveV2Settings,
  scanV2Repos,
  setClickupToken,
  setGithubToken,
  setOpencodeKey,
  setOpencodeModel,
  setT3Mapping,
  type ImportScope,
  type ProviderState,
  type ProviderStatus,
  type ProviderStatusMap,
  type T3MappingStatus,
  type T3TargetKind,
} from "../settings";
import type { RepoCandidate } from "../types";
import type { UpdateState } from "../updater";
import { Select, Switch } from "../ui/controls";
import { UpdateActions } from "./UpdateActions";

/* The dedicated v2 Settings view. Each provider owns its state and its errors: a
   failing probe or write leaves the other sections and the updater usable. Values entered
   by the user go only to a write command through a password field and are cleared on
   submit; the backend returns masks, never the value itself. */

const LABELS: Record<ProviderKey, string> = {
  github: "GitHub",
  clickup: "ClickUp",
  slack: "Slack",
  opencode: "OpenCode",
  t3: "T3 Code",
};

const STATE_LABEL: Record<ProviderState, string> = {
  missingSetup: "Set up",
  configuredUnverified: "Unverified",
  verified: "Connected",
  unavailable: "Unavailable",
};

function parseScope(raw: string | null, projects: V2Project[]): ImportScope {
  if (raw) {
    try {
      const parsed = JSON.parse(raw) as ImportScope;
      if (parsed.workspaceId && parsed.projectId) return parsed;
    } catch {
      // A malformed scope falls back to the empty form rather than blanking Settings.
    }
  }
  return { workspaceId: "", spaceId: null, projectId: projects[0]?.id ?? "" };
}

interface Props {
  provider: ProviderKey | null;
  version: string;
  update: UpdateState;
}

export function SettingsView({ provider, version, update }: Props) {
  const [statuses, setStatuses] = useState<ProviderStatusMap>({});
  const [projects, setProjects] = useState<V2Project[]>([]);
  const [settings, setSettings] = useState<V2Settings | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const { summon } = useKeybinds();
  const [chordDraft, setChordDraft] = useState(summon);
  const sections = useRef<Partial<Record<ProviderKey, HTMLElement | null>>>({});

  const load = useCallback(async () => {
    const [workspace, providerStatus] = await Promise.allSettled([
      v2WorkspaceSnapshot(),
      getProviderStatus(),
    ]);
    if (workspace.status === "fulfilled") {
      setProjects(workspace.value.projects);
      setSettings(workspace.value.settings);
    } else {
      setNotice(`Workspace settings could not be read: ${errorText(workspace.reason)}`);
    }
    if (providerStatus.status === "fulfilled") setStatuses(providerStatus.value);
  }, []);

  useEffect(() => {
    void load();
    const offStatus = onProviderStatus(setStatuses);
    const offChange = onSettingsChange(() => void load());
    const onFocus = () => void load();
    window.addEventListener("focus", onFocus);
    return () => {
      offStatus();
      offChange();
      window.removeEventListener("focus", onFocus);
    };
  }, [load]);

  // Discover providers from the running machine as soon as Settings opens. The Rust side
  // bounds each probe and publishes through the status event; this call just asks for it.
  useEffect(() => {
    void refreshProviders().then(setStatuses).catch(() => {});
  }, []);

  useEffect(() => setChordDraft(summon), [summon]);

  // A recovery route from a Task or Reference both scrolls to and focuses the section.
  useEffect(() => {
    if (!provider) return;
    const section = sections.current[provider];
    section?.scrollIntoView({ block: "start" });
    section?.focus({ preventScroll: true });
  }, [provider, statuses]);

  const status = (key: ProviderKey): ProviderStatus | undefined => statuses[key];
  const section = (key: ProviderKey, children: React.ReactNode) => (
    <section
      className="psec"
      id={`provider-${key}`}
      tabIndex={-1}
      aria-label={LABELS[key]}
      ref={(el) => {
        sections.current[key] = el;
      }}
    >
      <header className="psec-head">
        <h3>{LABELS[key]}</h3>
        <StatusMark status={status(key)} />
      </header>
      {status(key)?.reason && <p className="meta psec-reason">{status(key)?.reason}</p>}
      {status(key)?.lastFour && (
        <p className="meta mask">Stored credential {status(key)?.lastFour}</p>
      )}
      {children}
    </section>
  );

  const refresh = async () => {
    try {
      setStatuses(await refreshProviders());
    } catch (error) {
      setNotice(`Provider refresh failed: ${errorText(error)}`);
    }
  };

  const saveChord = async () => {
    try {
      await setKeybinds({ summon: chordDraft });
      setNotice("Summon shortcut saved.");
    } catch (error) {
      setNotice(`Could not save the shortcut: ${errorText(error)}`);
    }
  };

  return (
    <section className="main settings" aria-label="Settings">
      <header className="head">
        <div>
          <h1>Settings</h1>
          <div className="date meta">Relay {version}</div>
        </div>
      </header>
      <div className="scroll settings-scroll">
        <h2 className="sub" id="settings-updates">
          Updates
        </h2>
        <div className="setrow">
          <span>
            {update.kind === "error"
              ? `Update failed: ${update.message}`
              : `Relay ${version}${update.kind === "idle" ? " is up to date" : ""}`}
          </span>
          <span className="push" />
          <UpdateActions update={update} />
        </div>

        <h2 className="sub">Keybinds</h2>
        <div className="setrow">
          <label htmlFor="summon-chord">Summon</label>
          <input
            id="summon-chord"
            value={chordDraft}
            onChange={(event) => setChordDraft(event.target.value)}
            spellCheck={false}
            aria-label="Summon shortcut"
          />
          <button type="button" className="btn" onClick={() => void saveChord()}>
            Save
          </button>
        </div>
        <p className="meta psec-intro">
          Stored in keybinds.json and applied without a restart. Use a shortcut that is not
          taken by Windows or another app, for example alt+space or ctrl+alt+r.
        </p>

        <h2 className="sub">Providers</h2>
        <p className="meta psec-intro">
          Status and recovery destinations for the providers Relay uses. Credentials stay in
          Windows Credential Manager; only a masked last-four value is ever shown here.
        </p>
        <div className="psec-grid">
          {section("github", <GithubSection status={status("github")} onSaved={load} />)}
          {section("clickup", <ClickupSection status={status("clickup")} onSaved={load} />)}
          {section(
            "slack",
            <SlackSection status={status("slack")} onRefresh={() => void refresh()} />,
          )}
          {section("opencode", <OpencodeSection status={status("opencode")} onSaved={load} />)}
          {section("t3", <T3Section projects={projects} status={status("t3")} />)}
        </div>

        <h2 className="sub">Import</h2>
        <ImportSection projects={projects} settings={settings} onSaved={load} />

        <h2 className="sub">Repositories</h2>
        <RepoSection onSaved={load} />

        {notice && (
          <div className="set-msg" role="status">
            <span>{notice}</span>
            <button type="button" className="ib" title="Dismiss" onClick={() => setNotice(null)}>
              ×
            </button>
          </div>
        )}
      </div>
    </section>
  );
}

function StatusMark({ status }: { status?: ProviderStatus }) {
  const state = status?.status ?? "missingSetup";
  return (
    <span className={`pst pst-${state}`} role="status">
      {STATE_LABEL[state]}
    </span>
  );
}

/* GitHub keeps the stored-token-over-gh precedence. A detected gh login is labelled as
   an external session; a v2 token is checked when it is saved, never by starting PR sync. */
function GithubSection({ status, onSaved }: { status?: ProviderStatus; onSaved: () => Promise<void> }) {
  const [token, setToken] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    setBusy(true);
    setMessage(null);
    // A pasted credential is one-shot: it is cleared on submission, success or failure.
    const submitted = token;
    setToken("");
    try {
      await setGithubToken(submitted);
      await onSaved();
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };
  const clear = async () => {
    setBusy(true);
    try {
      await clearGithubToken();
      await onSaved();
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="pform">
      {status?.source === "externalSession" && (
        <p className="meta">Using an external gh session. A stored token takes precedence.</p>
      )}
      <label className="pfield">
        <span>Access token</span>
        <input
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={token}
          placeholder="Paste a GitHub token"
          onChange={(e) => setToken(e.target.value)}
        />
      </label>
      <div className="pactions">
        <button type="button" className="btn pri" disabled={busy || !token.trim()} onClick={() => void submit()}>
          Save token
        </button>
        {status?.lastFour && (
          <button type="button" className="btn" disabled={busy} onClick={() => void clear()}>
            Remove stored token
          </button>
        )}
      </div>
      {message && <p className="perr" role="alert">{message}</p>}
    </div>
  );
}

function ClickupSection({ status, onSaved }: { status?: ProviderStatus; onSaved: () => Promise<void> }) {
  const [token, setToken] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    setBusy(true);
    setMessage(null);
    const submitted = token;
    setToken("");
    try {
      await setClickupToken(submitted);
      await onSaved();
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };
  const clear = async () => {
    setBusy(true);
    try {
      await clearClickupToken();
      await onSaved();
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="pform">
      <label className="pfield">
        <span>API token</span>
        <input
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={token}
          placeholder="Paste a ClickUp token"
          onChange={(e) => setToken(e.target.value)}
        />
      </label>
      <div className="pactions">
        <button type="button" className="btn pri" disabled={busy || !token.trim()} onClick={() => void submit()}>
          Save token
        </button>
        {status?.lastFour && (
          <button type="button" className="btn" disabled={busy} onClick={() => void clear()}>
            Remove stored token
          </button>
        )}
      </div>
      {message && <p className="perr" role="alert">{message}</p>}
    </div>
  );
}

/* Slack has no token here. Claude is detected through the existing finder and its
   sign-in comes from `claude auth status --json`. A signed-in Claude leaves the
   connector unverified until connector results are available. */
function SlackSection({ status, onRefresh }: { status?: ProviderStatus; onRefresh: () => void }) {
  return (
    <div className="pform">
      <p className="meta">
        Enable the Slack connector in Claude Code, then refresh. A signed-in Claude by itself
        does not confirm the connector.
      </p>
      <div className="pactions">
        <button type="button" className="btn" onClick={onRefresh}>
          <Icon name="refresh" small />
          Refresh sign-in
        </button>
      </div>
      {status?.status === "unavailable" && (
        <p className="perr" role="alert">Claude sign-in status could not be read.</p>
      )}
    </div>
  );
}

/* Model and key are separate settings. The model is a non-secret provider/model string
   in v2 SQLite; the key is a v2 keyring entry. Neither is verified until a real adapter
   reports a result. */
function OpencodeSection({ status, onSaved }: { status?: ProviderStatus; onSaved: () => Promise<void> }) {
  const [model, setModel] = useState("");
  const [key, setKey] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const save = async (action: (value: string) => Promise<unknown>, value: string, clear: () => void) => {
    setBusy(true);
    setMessage(null);
    clear();
    try {
      await action(value);
      await onSaved();
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="pform">
      <label className="pfield">
        <span>Model</span>
        <input
          type="text"
          spellCheck={false}
          value={model}
          placeholder="opencode/gpt-5-nano"
          onChange={(e) => setModel(e.target.value)}
        />
      </label>
      <div className="pactions">
        <button
          type="button"
          className="btn pri"
          disabled={busy || !model.trim()}
          onClick={() => void save(setOpencodeModel, model, () => setModel(""))}
        >
          Save model
        </button>
      </div>
      <label className="pfield">
        <span>Provider key (optional)</span>
        <input
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={key}
          placeholder="Paste a provider key"
          onChange={(e) => setKey(e.target.value)}
        />
      </label>
      <div className="pactions">
        <button
          type="button"
          className="btn"
          disabled={busy || !key.trim()}
          onClick={() => void save(setOpencodeKey, key, () => setKey(""))}
        >
          Save key
        </button>
        {status?.lastFour && (
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => void save(clearOpencodeKey, "", () => undefined)}
          >
            Remove stored key
          </button>
        )}
      </div>
      <p className="meta">Configuration is saved locally; a real OpenCode result is not available yet.</p>
      {message && <p className="perr" role="alert">{message}</p>}
    </div>
  );
}

/* The mapping is saved without calling T3, so a stopped T3 never blocks a valid
   configuration. The check is explicit and the target is passed as one process argument. */
function T3Section({ projects, status }: { projects: V2Project[]; status?: ProviderStatus }) {
  const [projectId, setProjectId] = useState("");
  const [kind, setKind] = useState<T3TargetKind>("id");
  const [value, setValue] = useState("");
  const [result, setResult] = useState<T3MappingStatus | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const current = projects.find((p) => p.id === projectId);
  useEffect(() => {
    if (!projectId && projects.length > 0) setProjectId(projects[0].id);
  }, [projects, projectId]);
  useEffect(() => {
    if (!current) return;
    const stored = current.t3Target?.split(/:(.*)/s);
    if (stored?.length === 3 && (stored[0] === "id" || stored[0] === "path")) {
      setKind(stored[0] as T3TargetKind);
      setValue(stored[1]);
    } else {
      setKind("id");
      setValue(current.t3Target ?? "");
    }
    setResult(null);
  }, [current?.id, current?.t3Target]);

  const save = async (target: { kind: T3TargetKind; value: string } | null) => {
    if (!projectId) return;
    setBusy(true);
    setMessage(null);
    try {
      await setT3Mapping(projectId, target);
      setResult(null);
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  const check = async () => {
    if (!projectId) return;
    setBusy(true);
    setMessage(null);
    try {
      setResult(await checkT3Mapping(projectId));
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  const options = projects.map((p) => ({ value: p.id, label: p.name }));
  return (
    <div className="pform">
      <div className="pactions">
        <button type="button" className="btn" onClick={() => openT3()}>
          <Icon name="ext" small />
          Open T3 Code
        </button>
        {status?.source === "cli" && status.status === "missingSetup" && (
          <span className="meta">Start T3 Code and register the project there.</span>
        )}
      </div>
      {projects.length === 0 ? (
        <p className="meta">Register a repository below before mapping a project.</p>
      ) : (
        <>
          <label className="pfield">
            <span>Relay project</span>
            <Select value={projectId} options={options} onChange={setProjectId} />
          </label>
          <div className="pfield">
            <span>Target kind</span>
            <Select
              value={kind}
              options={[
                { value: "id", label: "T3 project ID" },
                { value: "path", label: "Absolute Windows path" },
              ]}
              onChange={(next) => setKind(next as T3TargetKind)}
            />
          </div>
          <label className="pfield">
            <span>{kind === "id" ? "Project ID" : "Path inside the T3 project"}</span>
            <input
              type="text"
              spellCheck={false}
              value={value}
              placeholder={kind === "id" ? "01JABCDEFGHJKLMNPQRSTVWXYZ" : "C:\\Projects\\sample-app"}
              onChange={(e) => setValue(e.target.value)}
            />
          </label>
          <div className="pactions">
            <button type="button" className="btn pri" disabled={busy || !value.trim()} onClick={() => void save({ kind, value })}>
              Save mapping
            </button>
            <button type="button" className="btn" disabled={busy || !current?.t3Target} onClick={() => void check()}>
              Check mapping
            </button>
            <button type="button" className="btn" disabled={busy || !current?.t3Target} onClick={() => void save(null)}>
              Clear
            </button>
          </div>
          {result && (
            <p className={result.status === "verified" ? "pok" : "perr"} role="status">
              {result.reason}
            </p>
          )}
        </>
      )}
      {message && <p className="perr" role="alert">{message}</p>}
    </div>
  );
}

/* Import preferences are saved on their own. Saving scope never starts a provider call;
   Import now is the single manual path and creates Notes, never Tasks. */
function ImportSection({
  projects,
  settings,
  onSaved,
}: {
  projects: V2Project[];
  settings: V2Settings | null;
  onSaved: () => Promise<void>;
}) {
  const [clickup, setClickup] = useState(false);
  const [github, setGithub] = useState(false);
  const [scope, setScope] = useState<ImportScope | null>(null);
  const [workspaces, setWorkspaces] = useState<{ id: string; name: string }[]>([]);
  const [spaces, setSpaces] = useState<{ id: string; name: string }[]>([]);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [runResult, setRunResult] = useState<ImportRunResult | null>(null);

  useEffect(() => {
    if (!settings) return;
    setClickup(settings.importClickupEnabled);
    setGithub(settings.importGithubEnabled);
    setScope(parseScope(settings.importScope, projects));
  }, [settings, projects]);

  useEffect(() => {
    if (!scope?.workspaceId) {
      setSpaces([]);
      return;
    }
    let gone = false;
    void clickupSpaces(scope.workspaceId)
      .then((list) => !gone && setSpaces(list))
      .catch(() => !gone && setSpaces([]));
    return () => {
      gone = true;
    };
  }, [scope?.workspaceId]);

  const loadWorkspaces = async () => {
    try {
      setWorkspaces(await clickupWorkspaces());
    } catch (error) {
      setMessage(errorText(error));
    }
  };

  const run = async () => {
    setBusy(true);
    setMessage(null);
    try {
      const result = await v2ImportNow();
      setRunResult(result);
      if (result.clickupError || result.githubError) {
        setMessage([result.clickupError, result.githubError].filter(Boolean).join(" "));
      }
      if (result.created > 0) await onSaved();
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    setBusy(true);
    setMessage(null);
    try {
      const complete = scope && scope.workspaceId && scope.projectId ? scope : null;
      await saveV2Settings({
        importClickupEnabled: clickup,
        importGithubEnabled: github,
        importScope: complete ? JSON.stringify(complete) : null,
      });
      await onSaved();
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="pform import-form">
      <label className="ui-check">
        <Switch checked={clickup} onChange={setClickup}>
          Cards assigned to you
        </Switch>
      </label>
      <label className="ui-check">
        <Switch checked={github} onChange={setGithub}>
          Open PRs in registered repos
        </Switch>
      </label>
      <div className="pfield">
        <span>ClickUp workspace</span>
        <div className="pactions">
          <Select
            value={scope?.workspaceId ?? ""}
            options={[
              { value: "", label: "Choose a workspace" },
              ...workspaces.map((w) => ({ value: w.id, label: w.name })),
            ]}
            onChange={(workspaceId) => setScope((s) => ({ ...(s ?? { spaceId: null, projectId: projects[0]?.id ?? "" }), workspaceId }))}
          />
          <button type="button" className="btn" onClick={() => void loadWorkspaces()}>
            Load workspaces
          </button>
        </div>
      </div>
      <div className="pfield">
        <span>Space (optional)</span>
        <Select
          value={scope?.spaceId ?? ""}
          options={[
            { value: "", label: "Whole workspace" },
            ...spaces.map((s) => ({ value: s.id, label: s.name })),
          ]}
          onChange={(spaceId) => setScope((s) => ({ ...(s ?? { workspaceId: "", projectId: "" }), spaceId: spaceId || null }))}
        />
      </div>
      <div className="pfield">
        <span>Relay project</span>
        <Select
          value={scope?.projectId ?? ""}
          options={[
            { value: "", label: "Choose a project" },
            ...projects.map((p) => ({ value: p.id, label: p.name })),
          ]}
          onChange={(projectId) => setScope((s) => ({ ...(s ?? { workspaceId: "", spaceId: null }), projectId }))}
        />
      </div>
      <div className="pactions">
        <button type="button" className="btn pri" disabled={busy} onClick={() => void save()}>
          Save Import settings
        </button>
        <button type="button" className="btn" disabled={busy} onClick={() => void run()}>
          {busy ? "Importing…" : "Import now"}
        </button>
      </div>
      {runResult && (
        <p className="meta" role="status">
          Created {runResult.created} Notes, skipped {runResult.skipped}
          {runResult.truncated ? " · ClickUp results were truncated" : ""}
        </p>
      )}
      <p className="meta">Enable a source, then run Import now. It creates one Note per assigned card or open PR, never a Task.</p>
      {message && <p className="perr" role="alert">{message}</p>}
    </div>
  );
}

/* Folder scanning stays read-only: it discovers GitHub origins with git. Registration
   creates a v2 project only. No PR polling, Slack reanalysis or one-shot importer runs. */
function RepoSection({ onSaved }: { onSaved: () => Promise<void> }) {
  const [folders, setFolders] = useState<string[]>([]);
  const [candidates, setCandidates] = useState<RepoCandidate[]>([]);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const pick = async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const choice = await open({ directory: true, multiple: false });
      if (typeof choice === "string" && choice && !folders.includes(choice)) {
        setFolders((all) => [...all, choice]);
      }
    } catch (error) {
      setMessage(errorText(error));
    }
  };

  const scan = async () => {
    setBusy(true);
    setMessage(null);
    try {
      setCandidates(await scanV2Repos(folders));
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  const register = async (candidate: RepoCandidate) => {
    setBusy(true);
    setMessage(null);
    try {
      await registerV2Repo(candidate.path, candidate.repo.owner, candidate.repo.name);
      await onSaved();
      setCandidates((all) => all.filter((c) => c.path !== candidate.path));
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="pform">
      <div className="pactions">
        <button type="button" className="btn" onClick={() => void pick()}>
          <Icon name="folders" small />
          Add folder
        </button>
        <button type="button" className="btn" disabled={busy || folders.length === 0} onClick={() => void scan()}>
          Scan for repos
        </button>
      </div>
      {folders.length > 0 && (
        <ul className="pfolders">
          {folders.map((folder) => (
            <li key={folder} className="meta">
              {folder}
            </li>
          ))}
        </ul>
      )}
      {candidates.map((candidate) => (
        <div className="setrow" key={candidate.path}>
          <span>
            <b>{candidate.repo.owner}/{candidate.repo.name}</b>{" "}
            <span className="meta path">{candidate.path}</span>
          </span>
          <span className="push" />
          <button
            type="button"
            className="btn"
            disabled={busy || !!candidate.projectId}
            onClick={() => void register(candidate)}
          >
            {candidate.projectId ? "Registered" : "Register"}
          </button>
        </div>
      ))}
      {message && <p className="perr" role="alert">{message}</p>}
    </div>
  );
}
