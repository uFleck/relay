import type { Snapshot, Status } from "../types";

/* Mock snapshot from design/main.html and design/hover.html. Times are relative to load so elapsed
   values, staleness and "today" stay believable whenever the dev build is opened. */

const loadedAt = Date.now();
const ago = (minutes: number) => new Date(loadedAt - minutes * 60_000).toISOString();

function dayAt(offsetDays: number, hours = 0, minutes = 0) {
  const d = new Date(loadedAt);
  d.setDate(d.getDate() + offsetDays);
  d.setHours(hours, minutes, 0, 0);
  return d.toISOString();
}

const fmtDay = (iso: string) =>
  new Date(iso).toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short" });
const fmtTime = (iso: string) =>
  new Date(iso).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });

/** The fixed default workflow (PLAN §0). Blocked sits off the main line, so the stepper skips it. */
export const defaultStatuses: Status[] = [
  { id: "backlog", name: "Backlog", short: "Back", category: "todo" },
  { id: "todo", name: "Todo", short: "Todo", category: "todo" },
  { id: "in_progress", name: "In Progress", short: "Prog", category: "active" },
  { id: "in_review", name: "In Review", short: "Rev", category: "review" },
  { id: "merged", name: "Merged", short: "Merged", category: "merged" },
  { id: "waiting_deploy", name: "Waiting for deploy", short: "Wait", category: "waiting" },
  { id: "deployed_rc", name: "Deployed to RC", short: "RC", category: "deploying" },
  { id: "done", name: "Done", short: "Done", category: "done" },
  { id: "blocked", name: "Blocked", short: "Blocked", category: "blocked" },
];

const checklistDue = dayAt(0, 17, 0);
const checklistProposedDue = dayAt(2, 12, 0);

export const mockSnapshot: Snapshot = {
  version: "0.4.1",
  inboxCount: 3,
  lastProposalHandledAt: ago(82),
  projects: [
    { id: "account", name: "Account", colorVar: "--i1" },
    { id: "portfolio", name: "Portfolio", colorVar: "--i2" },
    { id: "ops", name: "Ops", colorVar: "--i3" },
    { id: "relay", name: "Relay", colorVar: "--i4" },
  ],
  statuses: defaultStatuses,
  tasks: [
    {
      id: "t_checklist",
      ref: "SL-D0412",
      rev: 7,
      title: "Slack DM: deploy checklist",
      projectId: "account",
      statusId: "in_progress",
      due: { at: checklistDue, hasTime: true },
      claim: {
        id: "clm_01JD7M",
        holder: "OpenCode",
        claimedAt: ago(41),
        lastProgressAt: ago(9),
        lastProgress: "drafting rollback steps for billing worker",
        leaseMinutes: 30,
        releasedAt: null,
      },
      description:
        "Marcos asked for a written checklist before the billing worker deploy: migration order, the feature flag flip, and the rollback path if invoice generation stalls.",
      changes: [
        {
          id: "chg_checklist_1",
          kind: "requirement",
          at: ago(190),
          source: "slack",
          author: "Marcos Lima",
          text: "Put the rollback for the billing worker first, that's the part Bianca will need.",
          confidence: 0.83,
          permalink: "https://umbler.slack.com/archives/D04MLIMA/p1763390000",
          ackedAt: ago(170),
          ackedBy: "clm_01JD7M",
        },
      ],
      links: [
        { kind: "slack", label: "DM · Marcos Lima", state: "open", url: "https://umbler.slack.com/archives/D04MLIMA" },
        { kind: "thread", label: "OpenCode session", state: "live", url: "opencode://session/ses_8f2a" },
      ],
    },
    {
      id: "t_invoice",
      ref: "CU-86b4m19",
      rev: 3,
      title: "Invoice PDF shows due date one day early (UTC−3)",
      projectId: "account",
      statusId: "in_progress",
      due: { at: dayAt(1), hasTime: false },
      claim: null,
      description:
        "Invoices rendered after 21:00 BRT print the next day's due date minus one. The PDF renderer formats the UTC date instead of the account's time zone.",
      changes: [],
      links: [{ kind: "clickup", label: "ClickUp", state: "86b4m19", url: "https://app.clickup.com/t/86b4m19" }],
    },
    {
      id: "t_portfolio_deploy",
      ref: "GH-14",
      rev: 2,
      title: "Personal: portfolio deploy",
      projectId: "portfolio",
      statusId: "in_progress",
      due: { at: dayAt(3), hasTime: false },
      claim: null,
      description: "Ship the rebuilt portfolio to Vercel and move the old site to an archive subdomain.",
      changes: [],
      links: [
        { kind: "github", label: "Issue #14", state: "open", url: "https://github.com/uFleck/portfolio/issues/14" },
        { kind: "branch", label: "feat/astro-5", state: "+6", url: "https://github.com/uFleck/portfolio/tree/feat/astro-5" },
      ],
    },
    {
      id: "t_rc_auth",
      ref: "CU-86b4kx2",
      rev: 12,
      title: "RC build auth regression",
      projectId: "account",
      statusId: "merged",
      due: { at: dayAt(0), hasTime: false },
      claim: {
        id: "clm_01JD8Q",
        holder: "T3",
        claimedAt: ago(14),
        lastProgressAt: ago(2),
        lastProgress: "rerunning rc smoke on build 2026.11.17-rc3",
        leaseMinutes: 30,
        releasedAt: null,
      },
      description:
        "Login on the RC build loops back to `/signin` after the token refresh introduced in PR #211. Production is fine; RC uses the `rc.` cookie domain, so the refresh cookie is set on the wrong host.",
      changes: [
        {
          id: "chg_rc_3",
          kind: "requirement",
          at: ago(40),
          source: "slack",
          author: "Ana Ribeiro",
          text: "Also needs to work for the RC build.",
          confidence: 0.91,
          permalink: "https://umbler.slack.com/archives/C05ACCOUNT/p1763398920",
          ackedAt: null,
          ackedBy: null,
        },
        {
          id: "chg_rc_2",
          kind: "field",
          at: ago(55),
          source: "github",
          field: "status",
          from: "In Review",
          to: "Merged",
          ref: "PR #218",
        },
        {
          id: "chg_rc_1",
          kind: "requirement",
          at: ago(182),
          source: "slack",
          author: "Ana Ribeiro",
          text: "Refresh has to keep the remember-me cookie, support keeps getting tickets.",
          confidence: 0.86,
          permalink: "https://umbler.slack.com/archives/C05ACCOUNT/p1763390400",
          ackedAt: ago(170),
          ackedBy: "clm_01JD8Q",
        },
      ],
      links: [
        { kind: "pr", label: "PR #218", state: "merged", url: "https://github.com/ublr/account/pull/218" },
        { kind: "branch", label: "release/rc", state: "+2", url: "https://github.com/ublr/account/tree/release/rc" },
        { kind: "thread", label: "T3 thread", state: "live", url: "t3://thread/thr_rc_auth" },
        { kind: "clickup", label: "ClickUp", state: "86b4kx2", url: "https://app.clickup.com/t/86b4kx2" },
      ],
    },
    {
      id: "t_samesite",
      ref: "CU-86b4j7p",
      rev: 5,
      title: "Session cookie SameSite=Lax breaks billing iframe",
      projectId: "account",
      statusId: "waiting_deploy",
      due: { at: dayAt(1), hasTime: false },
      claim: null,
      description:
        "The billing iframe on the hosting panel loses the session since the cookie moved to `SameSite=Lax`. The fix sets `SameSite=None; Secure` for the billing path only.",
      changes: [],
      links: [
        { kind: "pr", label: "PR #215", state: "merged", url: "https://github.com/ublr/account/pull/215" },
        { kind: "clickup", label: "ClickUp", state: "86b4j7p", url: "https://app.clickup.com/t/86b4j7p" },
      ],
    },
    {
      id: "t_updater",
      ref: "GH-31",
      rev: 4,
      title: "Bump tauri-plugin-updater to 2.9 and re-sign latest.json",
      projectId: "relay",
      statusId: "deployed_rc",
      due: { at: dayAt(2), hasTime: false },
      claim: null,
      description:
        "Move to `tauri-plugin-updater` 2.9 and regenerate `latest.json` with the new minisign key so 0.4.2 installs over 0.4.1.",
      changes: [],
      links: [
        { kind: "github", label: "Issue #31", state: "open", url: "https://github.com/uFleck/relay/issues/31" },
        { kind: "pr", label: "PR #33", state: "merged", url: "https://github.com/uFleck/relay/pull/33" },
      ],
    },
    {
      id: "t_churn",
      ref: "CU-86b4f02",
      rev: 9,
      title: "Churn report CSV export — waiting on data team schema",
      projectId: "ops",
      statusId: "blocked",
      due: { at: dayAt(-1), hasTime: false },
      claim: {
        id: "clm_01JD2C",
        holder: "Claude Code",
        claimedAt: ago(60 * 52),
        lastProgressAt: ago(60 * 49),
        lastProgress: "waiting on churn_v2 schema from data team",
        leaseMinutes: 30,
        releasedAt: ago(60 * 48),
      },
      description:
        "Export the monthly churn report as CSV for finance. Blocked until the data team publishes the `churn_v2` view with the cancellation reason column.",
      changes: [],
      links: [
        { kind: "clickup", label: "ClickUp", state: "86b4f02", url: "https://app.clickup.com/t/86b4f02" },
        { kind: "slack", label: "#data-requests", state: "thread", url: "https://umbler.slack.com/archives/C02DATA" },
      ],
    },
    {
      id: "t_dns",
      ref: "GH-12",
      rev: 3,
      title: "Portfolio: custom domain DNS (registrar transfer lock)",
      projectId: "portfolio",
      statusId: "blocked",
      due: { at: dayAt(3), hasTime: false },
      claim: null,
      description: "The domain is under a 60-day transfer lock at the old registrar. Point DNS at Vercel once the lock lifts.",
      changes: [],
      links: [{ kind: "github", label: "Issue #12", state: "open", url: "https://github.com/uFleck/portfolio/issues/12" }],
    },
  ],
  proposals: [
    {
      id: "prp_01JD9A",
      kind: "change_deadline",
      taskId: "t_checklist",
      taskRev: 7,
      origin: "Slack DM",
      author: "Marcos Lima",
      episode: 4,
      messages: [
        { at: ago(11), who: "Marcos", parts: [{ text: "QA is out tomorrow, Bianca's covering the release desk alone" }] },
        {
          at: ago(10),
          who: "Marcos",
          parts: [
            { text: "can we push the deploy checklist to Thursday noon?", match: true },
            { text: " nothing on it ships before then anyway" },
          ],
        },
        { at: ago(6), who: "you", me: true, parts: [{ text: "fine by me, I'll move it" }] },
      ],
      jev: {
        stage: "stage B",
        ms: 640,
        scores: [
          { label: "affects existing", value: 0.88, threshold: 0.7, tone: "accent" },
          { label: "new task", value: 0.12, threshold: 0.8, tone: "muted" },
          { label: "needs human", value: 0.21, threshold: 0.6, tone: "warn" },
        ],
      },
      diff: [
        {
          field: "due",
          from: `${fmtDay(checklistDue)} ${fmtTime(checklistDue)}`,
          to: `${fmtDay(checklistProposedDue)} ${fmtTime(checklistProposedDue)}`,
        },
      ],
      patch: { due: { at: checklistProposedDue, hasTime: true } },
      note: "due changes are proposal-only",
    },
    {
      id: "prp_01JD9F",
      kind: "add_requirement",
      taskId: "t_samesite",
      taskRev: 5,
      origin: "#account-dev",
      author: "Ana Ribeiro",
      episode: 2,
      messages: [
        {
          at: ago(4),
          who: "Ana",
          parts: [
            { text: "before the deploy, " },
            { text: "the billing iframe fix also has to cover the Talk embed", match: true },
          ],
        },
      ],
      jev: {
        stage: "stage B",
        ms: 710,
        scores: [
          { label: "affects existing", value: 0.79, threshold: 0.7, tone: "accent" },
          { label: "new task", value: 0.18, threshold: 0.8, tone: "muted" },
          { label: "needs human", value: 0.34, threshold: 0.6, tone: "warn" },
        ],
      },
      diff: [{ field: "require", from: "", to: "Cover the Talk embed iframe too" }],
      patch: { requirement: "The billing iframe fix also has to cover the Talk embed." },
      note: "requirements wait for review below 0.80",
    },
  ],
  sync: [
    { name: "Slack", short: "Slack", state: "ok", reason: null },
    { name: "JEV", short: "JEV", state: "ok", reason: null },
    { name: "GitHub", short: "GH", state: "ok", reason: null },
    { name: "ClickUp", short: "ClickUp", state: "ok", reason: null },
    { name: "Executor", short: "Executor", state: "retrying", reason: "bridge reconnecting" },
  ],
  update: { state: "idle", version: "0.4.1", checkedAt: ago(7) },
};

export const mockUpdateReady: Snapshot["update"] = {
  state: "ready",
  version: "0.4.2",
  sizeMb: 11.8,
  signed: true,
  note: "Restart waits for the analyst job to finish · claims survive restart",
};
