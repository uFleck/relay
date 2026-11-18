import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { Icon } from "../components/Icon";
import {
  episodeAnalyze,
  episodeGet,
  episodesList,
  errorText,
  inTauri,
  jevClearKey,
  jevSetKey,
  jevStatus,
  jobRetry,
  jobsList,
  onEvent,
  openExternal,
  slackConnect,
  slackDisconnect,
  slackPeers,
  slackReanalyze,
  slackReanalyzeRecent,
  slackSetTransport,
  slackSetPeerExcluded,
  slackSyncNow,
  type AnalysisJob,
  type ReanalyzeReport,
  type EpisodeDetail,
  type SlackEpisode,
  type SlackMessage,
  type SlackStatus,
  type SlackSyncReport,
} from "../ipc";
import { elapsed, plural, useNow } from "../model";
import type { SyncState } from "../types";

/* Slack capture and JEV: the Settings sections, the Inbox episode list, the transcript sheet and the
   Review view's "From Slack" block. Nothing here sees a token after it is sent to Rust. */

const SLACK_APPS = "https://api.slack.com/apps";

/** Loads `load` now, on every `relay://change` and whenever the window regains focus. A reply that
    arrives after a newer request started is dropped. */
export function useLive<T>(load: () => Promise<T>, deps: React.DependencyList) {
  const [state, setState] = useState<{ data: T | undefined; error: string | null }>({ data: undefined, error: null });
  const seq = useRef(0);
  const reload = useCallback(load, deps);
  const run = useCallback(async () => {
    const n = ++seq.current;
    try {
      const data = await reload();
      if (n === seq.current) setState({ data, error: null });
    } catch (error) {
      if (n === seq.current) setState((s) => ({ data: s.data, error: errorText(error) }));
    }
  }, [reload]);
  useEffect(() => {
    void run();
    const off = onEvent("relay://change", () => void run());
    const onFocus = () => void run();
    window.addEventListener("focus", onFocus);
    return () => {
      off();
      window.removeEventListener("focus", onFocus);
    };
  }, [run]);
  const mutate = useCallback((fn: (data: T | undefined) => T) => {
    seq.current++;
    setState((s) => ({ data: fn(s.data), error: null }));
  }, []);
  return { ...state, reload: run, mutate };
}

export function Spinner() {
  return <span className="spin" aria-hidden="true" />;
}

/* ── formatting ── */

const EPISODE_STATE: Record<SlackEpisode["state"], { label: string; tone: string }> = {
  open: { label: "open", tone: "ok" },
  waiting: { label: "waiting", tone: "mut" },
  closed: { label: "closed", tone: "mut" },
  analyzed: { label: "analyzed", tone: "acc" },
  needs_review: { label: "needs review", tone: "warn" },
};

function EpisodeTag({ state }: { state: SlackEpisode["state"] }) {
  const s = EPISODE_STATE[state];
  return <span className={`tag ${s.tone}`}>{s.label}</span>;
}

const JOB_TONE: Record<AnalysisJob["state"], string> = {
  pending: "mut",
  running: "acc",
  pending_agent: "warn",
  done: "ok",
  failed: "bad",
  superseded: "mut",
  shadow: "mut",
};

/** Slack's `ts` is seconds since the epoch with a sequence suffix. */
const tsMs = (ts: string) => Math.round(Number(ts) * 1000);

const stamp = (ms: number) =>
  new Date(ms).toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });

const ago = (iso: string, now: number) => `${elapsed(iso, now)} ago`;

const peerLabel = (name: string | null, userId: string) => name ?? userId;

/** Short hints for the errors Slack returns on a bad token; the raw code stays visible next to them. */
const SLACK_HINTS: [RegExp, string][] = [
  [/invalid_auth/, "Slack rejected a token. Copy it again; reinstalling the app issues a new one."],
  [/missing_scope/, "The user token lacks a scope. Add im:read, im:history and users:read, then reinstall the app."],
  [/not_allowed_token_type/, "A token is the wrong kind: the first field takes xoxp-, the second xapp-."],
];

const slackHint = (error: string) => SLACK_HINTS.find(([re]) => re.test(error))?.[1] ?? null;

function tokenProblem(user: string, app: string) {
  if (!user.startsWith("xoxp-")) return "The user token starts with xoxp-. A bot token (xoxb-) cannot read your DMs.";
  if (!app.startsWith("xapp-")) return "The app token starts with xapp-. Generate it under Basic Information → App-Level Tokens.";
  return null;
}

function slackDot(s: SlackStatus): SyncState | "off" {
  if (s.connected) return "ok";
  if (s.error) return "down";
  return s.configured ? "retrying" : "off";
}

function slackLine(s: SlackStatus) {
  if (s.connected) return `Connected as ${s.user ?? "you"} in ${s.team ?? "Slack"}`;
  if (s.error) return s.configured ? "Slack connection failed" : "Not connected";
  return s.configured ? "Connecting…" : "Not connected";
}

function syncSummary(report: SlackSyncReport): string {
  if (report.messages === 0) return "Nothing new.";
  const analyzed = report.analyzed > 0 ? ` · ${plural(report.analyzed, "episode")} analyzed` : "";
  return `${plural(report.conversations, "conversation")} checked · ${plural(report.messages, "new message")}${analyzed}`;
}

function reanalyzeSummary(report: ReanalyzeReport): string {
  if (report.selected === 0) return "Nothing to redo: no recent conversation is waiting on a newer task.";
  return `${plural(report.selected, "conversation")} picked · ${report.analyzed} analyzed${report.failed > 0 ? ` · ${report.failed} failed` : ""}`;
}

/** Redoes the last 3 days of conversations that were screened before the tasks they mention existed. */
function ReanalyzeRow({ disabled }: { disabled: boolean }) {
  const [busy, setBusy] = useState(false);
  const [line, setLine] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function run() {
    setBusy(true);
    setLine(null);
    setError(null);
    try {
      const report = await slackReanalyzeRecent();
      setLine(reanalyzeSummary(report));
      if (report.error) setError(report.error);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <div className="setrow">
        <span>Conversations from the last 3 days that were backfilled or screened out are analyzed again with today's tasks.</span>
        <span className="push" />
        <button type="button" className="btn" disabled={disabled || busy} onClick={() => void run()}>
          {busy && <Spinner />}
          {busy ? "Re-analyzing…" : "Re-analyze last 3 days"}
        </button>
      </div>
      {line && (
        <p className="mcp-note" role="status">
          {line}
        </p>
      )}
      {error && <p className="mcp-err" role="alert">{error}</p>}
    </>
  );
}

function ErrorText({ error }: { error: string }) {
  const hint = slackHint(error);
  return (
    <p className="mcp-err" role="alert">
      {hint && <span className="hint">{hint} </span>}
      {error}
    </p>
  );
}

/* ── Settings → Slack ── */

interface SlackSettingsProps {
  status: SlackStatus | null;
  onStatus: (status: SlackStatus) => void;
}

export function SlackSettings({ status, onStatus }: SlackSettingsProps) {
  const now = useNow();
  const [userToken, setUserToken] = useState("");
  const [appToken, setAppToken] = useState("");
  const [busy, setBusy] = useState<"connect" | "disconnect" | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [syncReport, setSyncReport] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function connect(e: FormEvent) {
    e.preventDefault();
    const user = userToken.trim();
    const app = appToken.trim();
    const problem = inTauri ? tokenProblem(user, app) : "Slack capture runs in the desktop app.";
    if (problem) {
      setError(problem);
      return;
    }
    setBusy("connect");
    setError(null);
    try {
      const next = await slackConnect({ userToken: user, appToken: app });
      onStatus(next);
      if (next.error) setError(next.error);
      else {
        setUserToken("");
        setAppToken("");
      }
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  }

  async function disconnect() {
    setBusy("disconnect");
    setError(null);
    try {
      onStatus(await slackDisconnect());
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  }

  async function syncNow() {
    setSyncing(true);
    setSyncReport(null);
    setError(null);
    try {
      const report = await slackSyncNow();
      setSyncReport(syncSummary(report));
      if (report.error) setError(report.error);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setSyncing(false);
    }
  }

  async function chooseTransport(next: "claude" | "token") {
    setError(null);
    try {
      onStatus(await slackSetTransport(next));
    } catch (err) {
      setError(errorText(err));
    }
  }

  if (!status) return <p className="empty">Reading Slack status…</p>;
  const shown = error ?? status.error;
  if (status.transport === "claude" && status.claude) {
    const claude = status.claude;
    return (
      <>
        <TransportPicker value="claude" onChange={(next) => void chooseTransport(next)} />
        <div className="setrow" role="status">
          <i className={`dot sync-${!claude.found || claude.lastError ? "down" : claude.lastRunAt ? "ok" : "off"}`} />
          <span>
            {!claude.found
              ? "Claude Code not found"
              : claude.lastRunAt
                ? `Last sync ${ago(claude.lastRunAt, now)} · ${plural(claude.lastMessages, "message")}`
                : "No sync yet"}
          </span>
          <span className="push" />
          <button
            type="button"
            className="btn"
            disabled={!claude.found || syncing || claude.running}
            onClick={() => void syncNow()}
          >
            {(syncing || claude.running) && <Spinner />}
            {syncing || claude.running ? "Checking…" : "Sync now"}
          </button>
        </div>
        <p className="mcp-note">
          {claude.found ? "Claude Code found" : "Claude Code not found"} · last run ${claude.lastCostUsd.toFixed(3)} · today $
          {claude.costTodayUsd.toFixed(3)}
          {claude.lastDurationMs > 0 && ` · ${(claude.lastDurationMs / 1000).toFixed(0)} s`}
          {status.capturePaused && " · capture paused"}
        </p>
        <p className="mcp-note">
          Reads your 1:1 and group DMs through the claude.ai Slack connector (headless Claude, haiku) every 10 minutes
          and on Ctrl+Alt+S. No Slack app token needed.
        </p>
        {syncReport && (
          <p className="mcp-note" role="status">
            {syncReport}
          </p>
        )}
        {(error ?? claude.lastError) && <ErrorText error={(error ?? claude.lastError) as string} />}
        <ReanalyzeRow disabled={syncing || claude.running} />
      </>
    );
  }
  return (
    <>
      <TransportPicker value="token" onChange={(next) => void chooseTransport(next)} />
      <div className="setrow" role="status">
        <i className={`dot sync-${slackDot(status)}`} />
        <span>{slackLine(status)}</span>
        <span className="push" />
        {status.configured && (
          <>
            <button type="button" className="btn" disabled={busy !== null || syncing} onClick={() => void syncNow()}>
              {syncing && <Spinner />}
              {syncing ? "Checking…" : "Sync now"}
            </button>
            <button
              type="button"
              className="btn bad"
              disabled={busy !== null || syncing}
              onClick={() => void disconnect()}
            >
              {busy === "disconnect" ? "Disconnecting…" : "Disconnect"}
            </button>
          </>
        )}
      </div>
      {status.configured && (
        <p className="mcp-note">
          {plural(status.peers, "peer")} · {plural(status.openEpisodes, "open episode")}
          {status.lastEventAt && ` · last event ${ago(status.lastEventAt, now)}`}
          {status.capturePaused && " · capture paused"}
        </p>
      )}
      {status.configured && <p className="mcp-note">Ctrl+Alt+S checks for new DMs from anywhere.</p>}
      {syncReport && (
        <p className="mcp-note" role="status">
          {syncReport}
        </p>
      )}
      {shown && <ErrorText error={shown} />}
      {status.configured && <ReanalyzeRow disabled={busy !== null || syncing} />}

      {(!status.configured || status.error) && (
        <form className="sform" onSubmit={(e) => void connect(e)}>
          <label htmlFor="slack-user">User token</label>
          <input
            id="slack-user"
            className="inp mono"
            type="password"
            autoComplete="off"
            spellCheck={false}
            placeholder="xoxp-…"
            value={userToken}
            onChange={(e) => setUserToken(e.target.value)}
          />
          <label htmlFor="slack-app">App token</label>
          <input
            id="slack-app"
            className="inp mono"
            type="password"
            autoComplete="off"
            spellCheck={false}
            placeholder="xapp-…"
            value={appToken}
            onChange={(e) => setAppToken(e.target.value)}
          />
          <span />
          <div className="sform-f">
            <button type="submit" className="btn pri" disabled={busy !== null || !userToken.trim() || !appToken.trim()}>
              {busy === "connect" && <Spinner />}
              {busy === "connect" ? "Connecting…" : "Connect"}
            </button>
          </div>
        </form>
      )}
      <p className="mcp-note">
        DMs only; tokens stay in Windows Credential Manager; see <code>docs/SLACK_SETUP.md</code>. Create the app at{" "}
        <button type="button" className="linkish" onClick={() => void openExternal(SLACK_APPS)}>
          api.slack.com/apps
        </button>
        .
      </p>
      {status.configured && <PeerList count={status.peers} />}
    </>
  );
}

function TransportPicker({
  value,
  onChange,
}: {
  value: "claude" | "token";
  onChange: (next: "claude" | "token") => void;
}) {
  return (
    <div className="setrow">
      <label htmlFor="slack-transport">Transport</label>
      <span className="push" />
      <select
        id="slack-transport"
        className="inp"
        value={value}
        onChange={(e) => onChange(e.target.value as "claude" | "token")}
      >
        <option value="claude">Claude (claude.ai Slack connector)</option>
        <option value="token">Slack app token</option>
      </select>
    </div>
  );
}

function PeerList({ count }: { count: number }) {
  const { data: peers, error, mutate } = useLive(slackPeers, [count]);
  const [pending, setPending] = useState<ReadonlySet<string>>(new Set());
  const [failed, setFailed] = useState<string | null>(null);

  async function setExcluded(peerId: string, excluded: boolean) {
    setPending((s) => new Set(s).add(peerId));
    setFailed(null);
    try {
      const next = await slackSetPeerExcluded({ peerId, excluded });
      if (next) mutate((list = []) => list.map((p) => (p.id === next.id ? next : p)));
    } catch (err) {
      setFailed(errorText(err));
    } finally {
      setPending((s) => {
        const n = new Set(s);
        n.delete(peerId);
        return n;
      });
    }
  }

  return (
    <>
      <h3 className="sub">DM peers</h3>
      {error && <p className="mcp-err">{error}</p>}
      {failed && <p className="mcp-err">Could not change the peer: {failed}</p>}
      {peers?.length === 0 && <p className="empty">No DMs captured yet.</p>}
      {peers && peers.length > 0 && (
        <ul className="peers">
          {peers.map((p) => (
            <li key={p.id}>
              <span className={`ellip${p.excluded ? " muted-fg" : ""}`}>{peerLabel(p.name, p.userId)}</span>
              {p.isBot && <span className="tag mut">bot</span>}
              <span className="meta mono">{p.userId}</span>
              <span className="push" />
              <label className="meta">
                <input
                  type="checkbox"
                  checked={p.excluded}
                  disabled={pending.has(p.id)}
                  onChange={(e) => void setExcluded(p.id, e.target.checked)}
                />{" "}
                Exclude
              </label>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

/* ── Settings → JEV ── */

export function JevSettings() {
  const { data: status, error: loadError, mutate } = useLive(jevStatus, []);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState<"save" | "clear" | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function save(e: FormEvent) {
    e.preventDefault();
    if (!inTauri) {
      setError("JEV runs in the desktop app.");
      return;
    }
    setBusy("save");
    setError(null);
    try {
      const next = await jevSetKey(key.trim());
      mutate(() => next);
      if (next.error) setError(next.error);
      else setKey("");
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
      const next = await jevClearKey();
      mutate(() => next);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  }

  if (!status) return loadError ? <p className="mcp-err">{loadError}</p> : <p className="empty">Reading JEV status…</p>;
  const shown = error ?? status.error;
  return (
    <>
      <div className="setrow" role="status">
        <i className={`dot sync-${status.configured ? (status.error ? "down" : "ok") : "off"}`} />
        <span>{status.configured ? `API key saved${status.model ? ` · ${status.model}` : ""}` : "No API key"}</span>
        <span className="push" />
        {status.configured && (
          <button type="button" className="btn bad" disabled={busy !== null} onClick={() => void clear()}>
            {busy === "clear" ? "Clearing…" : "Clear"}
          </button>
        )}
      </div>
      {shown && <p className="mcp-err" role="alert">{shown}</p>}
      <form className="sform" onSubmit={(e) => void save(e)}>
        <label htmlFor="jev-key">{status.configured ? "Replace key" : "API key"}</label>
        <input
          id="jev-key"
          className="inp mono"
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={key}
          onChange={(e) => setKey(e.target.value)}
        />
        <span />
        <div className="sform-f">
          <button type="submit" className="btn pri" disabled={busy !== null || !key.trim()}>
            {busy === "save" && <Spinner />}
            {busy === "save" ? "Checking key…" : "Save"}
          </button>
        </div>
      </form>
      <p className="mcp-note">Save checks the key with one real JEV call. Relay never shows a saved key again.</p>
    </>
  );
}

/* ── Inbox ── */

const FILTERS = [
  { label: "All", state: undefined },
  { label: "Open", state: "open" },
  { label: "Needs review", state: "needs_review" },
  { label: "Analyzed", state: "analyzed" },
] as const;

type Filter = (typeof FILTERS)[number]["state"];

interface InboxProps {
  slack: SlackStatus | null;
  openId: string | null;
  onOpen: (episodeId: string) => void;
  onSettings: () => void;
}

export function InboxView({ slack, openId, onOpen, onSettings }: InboxProps) {
  const now = useNow();
  const [filter, setFilter] = useState<Filter>(undefined);
  const { data: episodes, error } = useLive(() => episodesList({ stateFilter: filter, limit: 50 }), [filter]);
  const notConnected = slack !== null && !slack.configured;
  // Captured episodes stay readable after Slack is disconnected, so "not connected" only replaces an empty list.
  const sub = !episodes
    ? "Loading episodes…"
    : episodes.length === 0 && notConnected
      ? "Slack is not connected"
      : `${plural(episodes.length, "captured episode")}${episodes.length === 50 ? " · newest 50" : ""}`;

  return (
    <section className="main">
      <header className="head">
        <div>
          <h1>Inbox</h1>
          <div className="date meta">{sub}</div>
        </div>
        <span className="push" />
        <div className="seg" role="tablist" aria-label="Episode state">
          {FILTERS.map((f) => (
            <button
              key={f.label}
              type="button"
              role="tab"
              aria-selected={filter === f.state}
              className={filter === f.state ? "on" : undefined}
              onClick={() => setFilter(f.state)}
            >
              {f.label}
            </button>
          ))}
        </div>
      </header>
      <div className="scroll">
        {error && <p className="mcp-err">Could not read episodes: {error}</p>}
        {episodes?.length === 0 &&
          (notConnected ? (
            <div className="blank">
              <b>Slack is not connected</b>
              <p>
                Relay captures your 1:1 DMs once Slack is connected.{" "}
                <button type="button" className="linkish" onClick={onSettings}>
                  Open Settings
                </button>
              </p>
            </div>
          ) : (
            <div className="blank">
              <b>{filter ? "No episodes in this state" : "No episodes yet"}</b>
              <p>
                {filter
                  ? "Pick another tab to see the rest."
                  : "Slack is connected. A DM starts an episode; it shows here as soon as the first message arrives."}
              </p>
            </div>
          ))}
        {episodes?.map(({ episode: e, peerName, peerUserId, messageCount, preview }) => (
          <button
            type="button"
            key={e.id}
            className={`erow${e.id === openId ? " sel" : ""}`}
            onClick={() => onOpen(e.id)}
          >
            <b className="ellip">{peerLabel(peerName, peerUserId)}</b>
            <EpisodeTag state={e.state} />
            <span className="meta ellip" title={preview ?? undefined}>
              {preview ?? "No text"}
            </span>
            <span className="meta nowrap">{plural(messageCount, "msg")}</span>
            <span className="meta nowrap">{ago(e.lastMessageAt, now)}</span>
          </button>
        ))}
      </div>
    </section>
  );
}

/* ── transcript sheet ── */

const newest = (jobs: AnalysisJob[]) =>
  jobs.reduce<AnalysisJob | null>((a, j) => (!a || Date.parse(j.createdAt) > Date.parse(a.createdAt) ? j : a), null);

interface SheetProps {
  id: string;
  onClose: () => void;
}

/** The transcript column. `?episode=` drives it and Esc (in App) closes it. */
export function EpisodeSheet({ id, onClose }: SheetProps) {
  const now = useNow();
  const sheetRef = useRef<HTMLElement>(null);
  const episode = useLive(() => episodeGet(id), [id]);
  const { data: detail, error } = episode;
  const jobs = useLive(() => jobsList({ limit: 50 }), []);
  const [queued, setQueued] = useState<AnalysisJob | null>(null);
  const [analyzing, setAnalyzing] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const [reanalyzing, setReanalyzing] = useState(false);
  const [analyzeError, setAnalyzeError] = useState<string | null>(null);

  useEffect(() => {
    sheetRef.current?.focus({ preventScroll: true });
  }, [id]);

  // The list is refreshed on every change, so it wins over the job `episodeAnalyze` returned once it has caught up.
  const listed = newest((jobs.data ?? []).filter((j) => j.conversationId === id));
  const job = listed && (!queued || Date.parse(listed.createdAt) >= Date.parse(queued.createdAt)) ? listed : queued;
  const running = analyzing || job?.state === "pending" || job?.state === "running";
  const waiting = job?.state === "pending_agent";

  async function retry(failed: AnalysisJob) {
    setRetrying(true);
    setAnalyzeError(null);
    try {
      const next = await jobRetry(failed.id);
      if (next) setQueued(next);
      else setAnalyzeError(inTauri ? "Relay did not requeue this job." : "Analysis runs in the desktop app.");
      await Promise.all([jobs.reload(), episode.reload()]);
    } catch (err) {
      setAnalyzeError(errorText(err));
    } finally {
      setRetrying(false);
    }
  }

  async function analyze() {
    setAnalyzing(true);
    setAnalyzeError(null);
    try {
      const next = await episodeAnalyze(id);
      if (next) setQueued(next);
      else setAnalyzeError(inTauri ? "Relay did not queue a job for this episode." : "Analysis runs in the desktop app.");
      void jobs.reload();
    } catch (err) {
      setAnalyzeError(errorText(err));
    } finally {
      setAnalyzing(false);
    }
  }

  async function reanalyze() {
    setReanalyzing(true);
    setAnalyzeError(null);
    try {
      const next = await slackReanalyze(id);
      if (next) setQueued(next);
      else setAnalyzeError(inTauri ? "Relay did not queue a job for this episode." : "Analysis runs in the desktop app.");
      await Promise.all([jobs.reload(), episode.reload()]);
    } catch (err) {
      setAnalyzeError(errorText(err));
    } finally {
      setReanalyzing(false);
    }
  }

  const name = detail ? peerLabel(detail.peer.name, detail.peer.userId) : "Episode";
  return (
    <aside ref={sheetRef} className="sheet" tabIndex={-1} aria-label={`Slack episode with ${name}`}>
      <div className="sh-h">
        <div className="sh-top">
          <Icon name="slack" small />
          {detail && <EpisodeTag state={detail.episode.state} />}
          <span className="push" />
          <button type="button" className="ib" title="Close (Esc)" onClick={onClose}>
            <Icon name="x" small />
          </button>
        </div>
        <h2>{name}</h2>
        {detail && (
          <div className="meta">
            {plural(detail.messages.length, "message")} · started {stamp(Date.parse(detail.episode.startedAt))} · last{" "}
            {ago(detail.episode.lastMessageAt, now)}
          </div>
        )}
      </div>

      <div className="sh-b">
        {error && <p className="mcp-err">Could not read the episode: {error}</p>}
        {detail === undefined && !error && <p className="desc">Loading…</p>}
        {detail === null && <p className="desc">This episode is gone. It may have been merged into another one.</p>}
        {detail && (
          <>
            {detail.episode.summary && (
              <section className="sec">
                <h3>Summary</h3>
                <p className="desc">{detail.episode.summary}</p>
              </section>
            )}
            <section className="sec">
              <h3>Transcript</h3>
              <Transcript detail={detail} name={name} />
            </section>
          </>
        )}
      </div>

      <div className="sh-f ejob">
        {analyzeError && <p className="mcp-err">{analyzeError}</p>}
        {job && <JobState job={job} retrying={retrying} onRetry={() => void retry(job)} />}
        <button
          type="button"
          className="btn pri"
          disabled={!detail || running || waiting || retrying || reanalyzing}
          onClick={() => void analyze()}
        >
          {running && <Spinner />}
          {analyzing ? "Queuing…" : running ? "Analysis running" : waiting ? "Waiting for an agent…" : "Analyze now"}
        </button>
        <button
          type="button"
          className="btn"
          title="Redo this conversation live with the tasks that exist now, replacing a backfill or screened-out result"
          disabled={!detail || running || waiting || retrying || analyzing || reanalyzing}
          onClick={() => void reanalyze()}
        >
          {reanalyzing && <Spinner />}
          {reanalyzing ? "Re-analyzing…" : "Re-analyze"}
        </button>
      </div>
    </aside>
  );
}

function Transcript({ detail, name }: { detail: EpisodeDetail; name: string }) {
  const messages = [...detail.messages].sort((a, b) => tsMs(a.ts) - tsMs(b.ts));
  if (messages.length === 0) return <p className="desc">No messages in this episode.</p>;
  return (
    <ol className="tx">
      {messages.map((m) => (
        <TranscriptLine key={m.id} m={m} name={name} />
      ))}
    </ol>
  );
}

function TranscriptLine({ m, name }: { m: SlackMessage; name: string }) {
  return (
    <li className={`${m.direction === "out" ? "me" : ""}${m.deleted ? " del" : ""}`}>
      <div className="tx-h">
        <b>{m.direction === "out" ? "You" : name}</b>
        <span className="meta">{stamp(tsMs(m.ts))}</span>
        {m.editedAt && <span className="meta">edited</span>}
        {m.historical && <span className="meta">backfill</span>}
        {m.deleted && <span className="tag mut">deleted</span>}
        <span className="push" />
        {m.permalink && (
          <button type="button" className="ib" title="Open in Slack" onClick={() => void openExternal(m.permalink!)}>
            <Icon name="ext" small />
          </button>
        )}
      </div>
      <p>{m.text || (m.deleted ? "Message deleted." : "No text.")}</p>
    </li>
  );
}

function JobState({ job, retrying, onRetry }: { job: AnalysisJob; retrying: boolean; onRetry: () => void }) {
  return (
    <div className="jstate" role="status">
      <div className="jstate-h">
        <span className={`tag ${JOB_TONE[job.state]}`}>{job.state.replace("_", " ")}</span>
        {job.route && <span className="meta">route {job.route.replace("_", " ")}</span>}
        <span className="meta">{job.kind}</span>
      </div>
      {job.state === "pending_agent" && (
        <p className="meta">Waiting for an agent. An analyst session picks this job up through Relay's analyst MCP endpoint.</p>
      )}
      {job.summary && <p>{job.summary}</p>}
      {job.state === "failed" ? (
        <div className="jstate-fail">
          <p className="mcp-err">{job.error ?? "The analysis failed without an error message."}</p>
          <button type="button" className="btn" disabled={retrying} onClick={onRetry}>
            {retrying && <Spinner />}
            {retrying ? "Requeueing…" : "Retry"}
          </button>
        </div>
      ) : (
        job.error && <p className="mcp-err">{job.error}</p>
      )}
    </div>
  );
}

/* ── Review → From Slack ── */

export function FromSlack({ onOpen }: { onOpen: (episodeId: string) => void }) {
  const now = useNow();
  const { data } = useLive(async () => {
    const jobs = (await jobsList({ stateFilter: "pending" })).filter((j) => j.route === "review");
    const ids = [...new Set(jobs.map((j) => j.conversationId))];
    const details = await Promise.all(ids.map((id) => episodeGet(id).catch(() => null)));
    const byId = new Map(details.flatMap((d) => (d ? [[d.episode.id, d] as const] : [])));
    return jobs.map((job) => ({ job, detail: byId.get(job.conversationId) ?? null }));
  }, []);
  if (!data?.length) return null;
  return (
    <>
      <h3 className="sub">From Slack · {data.length}</h3>
      <ul className="jrows">
        {data.map(({ job, detail }) => (
          <li key={job.id}>
            <div className="jrow-t">
              <span className="ellip">{job.summary ?? "No summary yet"}</span>
              <span className="meta">
                {detail ? peerLabel(detail.peer.name, detail.peer.userId) : "Unknown episode"} ·{" "}
                {ago(detail?.episode.lastMessageAt ?? job.createdAt, now)}
              </span>
            </div>
            <button type="button" className="btn" onClick={() => onOpen(job.conversationId)}>
              <Icon name="slack" small />
              Transcript
            </button>
          </li>
        ))}
      </ul>
    </>
  );
}
