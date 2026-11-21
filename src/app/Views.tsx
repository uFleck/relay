import { useState } from "react";
import { Icon } from "../components/Icon";
import { ProposalCard } from "../components/ProposalCard";
import type { HoverFlags, SlackStatus } from "../ipc";
import { claimState, clock, elapsed, liveClaims, plural, statusOf, useNow } from "../model";
import type { Proposal, ProposalDecision, Snapshot, StatusCategory, Task } from "../types";
import { checkForUpdates, downloadUpdate, installUpdate, type UpdateState } from "../updater";
import { GithubSettings, ProjectRepo } from "./Github";
import { ClickupSettings } from "./Import";
import { FromSlack, JevSettings, SlackSettings } from "./Slack";
import { TaskGroups } from "./Today";
import { Switch } from "../ui/controls";

/* The rail's views besides Today. Each reads the same snapshot; none invents data the snapshot lacks. */

function Head({ title, sub }: { title: string; sub: string }) {
  return (
    <header className="head">
      <div>
        <h1>{title}</h1>
        <div className="date meta">{sub}</div>
      </div>
    </header>
  );
}

function Blank({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="blank">
      <b>{title}</b>
      <p>{children}</p>
    </div>
  );
}

interface ProjectsProps {
  snap: Snapshot;
  selectedId: string | null;
  onSelect: (id: string) => void;
  /** The selected task, drawn in place under its row. */
  renderTask: (task: Task) => React.ReactNode;
}

/** Category order for the mini bars and counts; done sits last. */
const MIX: StatusCategory[] = ["active", "review", "merged", "waiting", "deploying", "blocked", "todo", "done"];

/** One project in focus: the list on the left, its repo and import sources as tiles, its tasks below. */
export function ProjectsView({ snap, selectedId, onSelect, renderTask }: ProjectsProps) {
  const selectedProject = snap.tasks.find((t) => t.id === selectedId)?.projectId;
  const [picked, setPicked] = useState<string | null>(null);
  const [tab, setTab] = useState<"tasks" | "import">("tasks");
  const current = snap.projects.find((p) => p.id === (picked ?? selectedProject)) ?? snap.projects[0];

  if (!current) {
    return (
      <section className="main">
        <Head title="Projects" sub="0 projects" />
        <Blank title="No projects">Projects come from Relay's database; none exist yet.</Blank>
      </section>
    );
  }

  const mixOf = (projectId: string) => {
    const counts = new Map<StatusCategory, number>();
    for (const t of snap.tasks) {
      if (t.projectId !== projectId) continue;
      const cat = statusOf(snap, t).category;
      counts.set(cat, (counts.get(cat) ?? 0) + 1);
    }
    return counts;
  };
  const mix = mixOf(current.id);
  const n = (cat: StatusCategory) => mix.get(cat) ?? 0;
  const total = [...mix.values()].reduce((a, b) => a + b, 0);
  const tasks = snap.tasks.filter((t) => t.projectId === current.id);
  const linked = (kind: "clickup" | "github") => tasks.filter((t) => t.links.some((l) => l.kind === kind)).length;
  const sources = [
    { name: "ClickUp", count: linked("clickup") },
    { name: "GitHub", count: linked("github") },
  ].filter((s) => s.count > 0);
  const pick = (id: string) => {
    setPicked(id);
    setTab("tasks");
  };

  return (
    <section className="main ws">
      <nav className="ws-side" aria-label="Projects">
        <h1>Projects</h1>
        {snap.projects.map((p) => {
          const m = mixOf(p.id);
          const all = [...m.values()].reduce((a, b) => a + b, 0);
          const open = all - (m.get("done") ?? 0);
          return (
            <button
              type="button"
              key={p.id}
              className="ws-pj"
              aria-current={p.id === current.id}
              style={{ "--pc": `var(${p.colorVar})` } as React.CSSProperties}
              onClick={() => pick(p.id)}
            >
              <i className="sw" />
              <b>{p.name}</b>
              <span className="meta">{all ? `${open} open` : "empty"}</span>
              <span className="mb">
                {MIX.filter((c) => m.get(c)).map((c) => (
                  <i key={c} style={{ flex: m.get(c), background: `var(--c-${c})` }} />
                ))}
              </span>
            </button>
          );
        })}
      </nav>

      <div className="ws-main" style={{ "--pc": `var(${current.colorVar})` } as React.CSSProperties}>
        <header className="ws-head">
          <div className="ws-title">
            <i className="sw" />
            <h2>{current.name}</h2>
          </div>
          <div className="ws-chips">
            <Chip cat="todo" n={total - n("done")} label="open" />
            <Chip cat="active" n={n("active")} label="in progress" />
            <Chip cat="blocked" n={n("blocked")} label="blocked" />
            <Chip cat="done" n={n("done")} label="done" />
          </div>
          <div className="ws-tiles">
            <div className="ws-tile">
              <div className="k">Repository</div>
              <ProjectRepo key={current.id} project={current} />
            </div>
            <div className="ws-tile">
              <div className="k">
                Import sources
                <span className="push" />
                <button type="button" className="btn ghost" onClick={() => setTab("import")}>
                  Import…
                </button>
              </div>
              {sources.length ? (
                sources.map((s) => (
                  <div className="src" key={s.name}>
                    <i className="dot sync-ok" />
                    <b>{s.name}</b>
                    <span className="meta">{plural(s.count, "linked task")}</span>
                  </div>
                ))
              ) : (
                <div className="meta">Nothing imported yet.</div>
              )}
            </div>
          </div>
          <div className="ws-tabs" role="tablist">
            <button type="button" role="tab" aria-selected={tab === "tasks"} onClick={() => setTab("tasks")}>
              Tasks
            </button>
            <button type="button" role="tab" aria-selected={tab === "import"} onClick={() => setTab("import")}>
              Import
            </button>
          </div>
        </header>
        <div className="scroll">
          {tab === "import" ? (
            <div className="ws-empty">
              <b>Import moved to Settings</b>
              <p>Enable a source and run Import now. Each imported item becomes one Note with its source Reference.</p>
            </div>
          ) : total === 0 ? (
            <div className="ws-empty">
              <b>{current.name} has no tasks yet</b>
              <p>Link the repository so agents can claim work here, import a ClickUp list or GitHub issues, or approve a Slack proposal for this project.</p>
              <button type="button" className="btn pri" onClick={() => setTab("import")}>
                Import…
              </button>
            </div>
          ) : (
            <TaskGroups
              snapshot={{ ...snap, tasks }}
              query=""
              selectedId={selectedId}
              onSelect={onSelect}
              renderSelected={renderTask}
              empty="Every task in this project is done."
            />
          )}
        </div>
      </div>
    </section>
  );
}

function Chip({ cat, n, label }: { cat: StatusCategory; n: number; label: string }) {
  return (
    <span className="chipc">
      <span className="dot" style={{ "--c": `var(--c-${cat})` } as React.CSSProperties} />
      <b>{n}</b>
      {label}
    </span>
  );
}

interface Activity {
  key: string;
  at: string;
  task: Task;
  who: string;
  text: string;
}

/** Claims, progress, releases and agent-made changes, newest first. */
function activity(tasks: Task[]): Activity[] {
  return tasks
    .flatMap((t) => {
      const out: Activity[] = [];
      const c = t.claim;
      if (c) {
        out.push({ key: `${c.id}:claim`, at: c.claimedAt, task: t, who: c.holder, text: "claimed" });
        if (c.lastProgressAt !== c.claimedAt) out.push({ key: `${c.id}:prog`, at: c.lastProgressAt, task: t, who: c.holder, text: c.lastProgress });
        if (c.releasedAt) out.push({ key: `${c.id}:rel`, at: c.releasedAt, task: t, who: c.holder, text: "released" });
      }
      for (const ch of t.changes) {
        if (ch.source !== "agent") continue;
        out.push({
          key: ch.id,
          at: ch.at,
          task: t,
          who: ch.kind === "requirement" ? ch.author : "agent",
          text: ch.kind === "requirement" ? `added “${ch.text}”` : `${ch.field} ${ch.from} → ${ch.to}`,
        });
      }
      return out;
    })
    .sort((a, b) => Date.parse(b.at) - Date.parse(a.at));
}

export function AgentsView({ snap, onSelect }: { snap: Snapshot; onSelect: (id: string) => void }) {
  const now = useNow();
  const live = liveClaims(snap.tasks).sort((a, b) => Date.parse(b.claim.claimedAt) - Date.parse(a.claim.claimedAt));
  const log = activity(snap.tasks).slice(0, 60);
  return (
    <section className="main">
      <Head title="Agents" sub={`${plural(live.length, "live claim")} · ${plural(log.length, "event")}`} />
      <div className="scroll">
        <h3 className="sub">Live claims</h3>
        {live.length === 0 ? (
          <p className="empty">No agent holds a claim. Claims show here when T3, OpenCode or Claude Code picks up a task.</p>
        ) : (
          live.map((t) => {
            const c = t.claim;
            const state = claimState(c, now);
            return (
              <button type="button" className="arow" key={c.id} onClick={() => onSelect(t.id)}>
                <span className={`live ${state}`} />
                <b>{c.holder}</b>
                <span className="t" title={t.title}>
                  {t.title}
                </span>
                <span className="meta ellip" title={c.lastProgress}>
                  {c.lastProgress}
                </span>
                <span className={`meta nowrap${state === "stale" ? " warn-fg" : ""}`}>
                  {elapsed(c.claimedAt, now)} · last {elapsed(c.lastProgressAt, now)} ago
                </span>
              </button>
            );
          })
        )}
        <h3 className="sub">Activity</h3>
        {log.length === 0 ? (
          <p className="empty">No agent activity yet.</p>
        ) : (
          <ol className="alog">
            {log.map((a) => (
              <li key={a.key}>
                <button type="button" onClick={() => onSelect(a.task.id)}>
                  <span className="meta">{clock(a.at)}</span>
                  <b>{a.who}</b>
                  <span className="ellip" title={a.text}>
                    {a.text}
                  </span>
                  <span className="meta ellip" title={a.task.title}>
                    {a.task.title}
                  </span>
                </button>
              </li>
            ))}
          </ol>
        )}
      </div>
    </section>
  );
}

interface ReviewProps {
  snap: Snapshot;
  current: Proposal | undefined;
  /** The proposal the route names, scrolled into view. */
  routed: string | null;
  busy: ReadonlySet<string>;
  onDecide: (p: Proposal, decision: ProposalDecision) => void;
  onEdit: (p: Proposal) => void;
  onOpenEpisode: (episodeId: string) => void;
}

export function ReviewView({ snap, current, routed, busy, onDecide, onEdit, onOpenEpisode }: ReviewProps) {
  const total = snap.proposals.length;
  return (
    <section className="main">
      <Head title="Review" sub={total ? `${plural(total, "proposal")} waiting · A / E / R act on the highlighted one` : "Proposals JEV raises from Slack"} />
      <div className="scroll">
        <FromSlack onOpen={onOpenEpisode} />
        {total === 0 && (
          <Blank title="Nothing to review">
            {snap.lastProposalHandledAt
              ? `The queue is clear. Last proposal handled at ${clock(snap.lastProposalHandledAt)}.`
              : "When JEV matches a Slack episode to a task, the proposal waits here for you."}
          </Blank>
        )}
        {snap.proposals.map((p, i) => (
          <ProposalCard
            key={p.id}
            size="full"
            proposal={p}
            task={snap.tasks.find((t) => t.id === p.taskId)}
            position={{ index: i + 1, total }}
            busy={busy.has(p.id)}
            current={p.id === current?.id}
            reveal={p.id === routed}
            onApprove={() => onDecide(p, "approve")}
            onEdit={() => onEdit(p)}
            onReject={() => onDecide(p, "reject")}
          />
        ))}
      </div>
    </section>
  );
}

interface SettingsProps {
  snap: Snapshot;
  update: UpdateState;
  flags: HoverFlags | null;
  onFlag: (flag: "dnd" | "paused", on: boolean) => Promise<void>;
  slack: SlackStatus | null;
  onSlack: (status: SlackStatus) => void;
}

export function SettingsView({ snap, update, flags, onFlag, slack, onSlack }: SettingsProps) {
  const [saving, setSaving] = useState<string | null>(null);
  const toggle = async (flag: "dnd" | "paused") => {
    if (!flags) return;
    setSaving(flag);
    await onFlag(flag, !flags[flag]);
    setSaving(null);
  };
  return (
    <section className="main">
      <Head title="Settings" sub={`Relay ${snap.version}`} />
      <div className="scroll">
        <h3 className="sub">Updates</h3>
        <div className="setrow">
          <span>{updateText(update, snap.version)}</span>
          <span className="push" />
          <UpdateActions update={update} />
        </div>

        <h3 className="sub">Hover</h3>
        {flags === null ? (
          <p className="empty">Hover settings are available in the desktop app.</p>
        ) : (
          <>
            <Switch className="setrow" checked={flags.dnd} disabled={saving === "dnd"} onChange={() => void toggle("dnd")}>
              <span>Do not disturb · stops pulses, keeps the badge</span>
              <span className="push" />
            </Switch>
            <Switch className="setrow" checked={flags.paused} disabled={saving === "paused"} onChange={() => void toggle("paused")}>
              <span>Pause capture</span>
              <span className="push" />
            </Switch>
          </>
        )}

        <h3 className="sub">Slack</h3>
        <SlackSettings status={slack} onStatus={onSlack} />

        <h3 className="sub">JEV</h3>
        <JevSettings />

        <h3 className="sub">GitHub</h3>
        <GithubSettings />

        <h3 className="sub">ClickUp</h3>
        <ClickupSettings projects={snap.projects} />

        <h3 className="sub">Sources</h3>
        {snap.sync.every((s) => s.state === "off") && (
          <p className="empty">Nothing is connected yet. Set up Slack, JEV, GitHub or ClickUp above.</p>
        )}
        {snap.sync.map((s) => (
          <div className="setrow" key={s.name}>
            <i className={`dot sync-${s.state}`} />
            <span>{s.name}</span>
            <span className="meta push">{s.reason ?? s.state}</span>
          </div>
        ))}
      </div>
    </section>
  );
}

function updateText(u: UpdateState, version: string) {
  switch (u.kind) {
    case "idle":
      return `Relay ${version} · up to date as far as the last check knows`;
    case "checking":
      return "Checking for updates…";
    case "available":
      return `Relay ${u.version} is available`;
    case "downloading":
      return `Downloading ${u.version}${u.progress === null ? "…" : ` · ${Math.round(u.progress * 100)}%`}`;
    case "ready":
      return `Relay ${u.version} is downloaded and signed`;
    case "error":
      return `Update failed: ${u.message}`;
  }
}

/** The action that fits the updater's state; the updater itself refuses overlapping work. */
export function UpdateActions({ update }: { update: UpdateState }) {
  if (update.kind === "available") {
    return (
      <button type="button" className="btn pri" onClick={() => void downloadUpdate()}>
        <Icon name="download" small />
        Download
      </button>
    );
  }
  if (update.kind === "ready") {
    return (
      <button type="button" className="btn pri" onClick={() => void installUpdate()}>
        Restart to update
      </button>
    );
  }
  const busy = update.kind === "checking" || update.kind === "downloading";
  return (
    <button type="button" className="btn" disabled={busy} onClick={() => void checkForUpdates(true)}>
      {update.kind === "checking" ? "Checking…" : update.kind === "error" ? "Try again" : "Check for updates"}
    </button>
  );
}

/** The rail's update notice: only when there is something to act on. */
export function UpdateNotice({ update }: { update: UpdateState }) {
  if (update.kind !== "available" && update.kind !== "downloading" && update.kind !== "ready") return null;
  return (
    <div className="unote" role="status">
      <span>
        <b>{update.version}</b>{" "}
        {update.kind === "ready"
          ? "ready"
          : update.kind === "available"
            ? "available"
            : `downloading${update.progress === null ? "…" : ` ${Math.round(update.progress * 100)}%`}`}
      </span>
      {update.kind !== "downloading" && <UpdateActions update={update} />}
    </div>
  );
}
