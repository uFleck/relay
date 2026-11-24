import type {
  V2Note,
  V2Project,
  V2Reference,
  V2Task,
  V2WorkspaceSnapshot,
} from "./ipc";
import type { Chip, V2PickerRow } from "./references";

const reference = (
  id: string,
  kind: string,
  targetId: string,
  canonicalUrl: string,
): V2Reference => ({
  id,
  kind,
  targetId,
  canonicalUrl,
  snapshotJson: null,
});

const projects: V2Project[] = [
  {
    id: "project-billing",
    name: "billing-api",
    slug: "billing-api",
    repoPath: null,
    ownerRepo: "billing-api",
    t3Target: null,
    createdAt: "2026-11-01T09:00:00",
    updatedAt: "2026-11-01T09:00:00",
  },
  {
    id: "project-web",
    name: "web-app",
    slug: "web-app",
    repoPath: null,
    ownerRepo: "web-app",
    t3Target: null,
    createdAt: "2026-11-01T09:00:00",
    updatedAt: "2026-11-01T09:00:00",
  },
];

const task = (
  id: string,
  noteId: string,
  sourceLineId: string,
  originalLine: string,
  agentMode: V2Task["agentMode"],
  status: V2Task["status"],
  projectId: string | null,
  createdAt: string,
): V2Task => ({
  id,
  sourceLineId,
  noteId,
  originalLine,
  agentMode,
  status,
  projectId,
  brief: null,
  grillTurn: null,
  pendingAction: null,
  failureStage: null,
  failureReason: null,
  threadUrl: null,
  references: [],
  createdAt,
  updatedAt: createdAt,
});

const notes: V2Note[] = [
  {
    id: "note-1",
    revision: 1,
    source: "import",
    body: "@billing-api#139 Add idempotency keys",
    createdAt: "2026-11-20T17:40:00",
    updatedAt: "2026-11-20T17:40:00",
    lines: [
      {
        id: "line-1",
        text: "@billing-api#139 Add idempotency keys",
        position: 0,
        references: [
          reference("ref-1", "github", "billing-api#139", "https://github.com/billing-api/pull/139"),
        ],
      },
    ],
    tasks: [],
  },
  {
    id: "note-2",
    revision: 1,
    source: "local",
    body: "should we drop the legacy export endpoint",
    createdAt: "2026-11-20T16:02:00",
    updatedAt: "2026-11-20T16:02:00",
    lines: [
      {
        id: "line-2",
        text: "should we drop the legacy export endpoint",
        position: 0,
        references: [],
      },
    ],
    tasks: [
      task("task-1", "note-2", "line-2", "drop the legacy export endpoint", "grill", "done", "project-web", "2026-11-20T16:02:00"),
    ],
  },
  {
    id: "note-3",
    revision: 1,
    source: "local",
    body: "Standup: retry storm on webhooks again, see @#support/1731000000",
    createdAt: "2026-11-21T09:12:00",
    updatedAt: "2026-11-21T09:12:00",
    lines: [
      {
        id: "line-3",
        text: "Standup: retry storm on webhooks again, see @#support/1731000000",
        position: 0,
        references: [
          reference("ref-2", "slack", "#support/1731000000", "https://slack.com/archives/support/p1731000000"),
        ],
      },
    ],
    tasks: [],
  },
  {
    id: "note-4",
    revision: 1,
    source: "local",
    body: "webhook retries hammer the queue @billing-api#142",
    createdAt: "2026-11-21T09:40:00",
    updatedAt: "2026-11-21T09:40:00",
    lines: [
      {
        id: "line-4",
        text: "webhook retries hammer the queue @billing-api#142",
        position: 0,
        references: [
          reference("ref-3", "github", "billing-api#142", "https://github.com/billing-api/pull/142"),
        ],
      },
    ],
    tasks: [
      task("task-2", "note-4", "line-4", "fix webhook retry backoff", "act", "dispatched", "project-billing", "2026-11-21T09:40:00"),
      task("task-3", "note-4", "line-4", "write a postmortem draft", "wait", "awaiting_approval", "project-billing", "2026-11-21T09:40:00"),
    ],
  },
  {
    id: "note-5",
    revision: 1,
    source: "local",
    body: "export is slow for big accounts @CU-7h2p9",
    createdAt: "2026-11-21T10:05:00",
    updatedAt: "2026-11-21T10:05:00",
    lines: [
      {
        id: "line-5",
        text: "export is slow for big accounts @CU-7h2p9",
        position: 0,
        references: [
          reference("ref-4", "clickup", "CU-7h2p9", "https://app.clickup.com/t/7h2p9"),
        ],
      },
    ],
    tasks: [
      task("task-4", "note-5", "line-5", "draft a migration plan", "wait", "awaiting_approval", "project-web", "2026-11-21T10:05:00"),
    ],
  },
];

export const visualFixtureWorkspace = (): V2WorkspaceSnapshot => {
  const visibleTasks = notes.flatMap((note) => note.tasks);
  const offscreenTask = task(
    "task-offscreen",
    "note-offscreen",
    "line-offscreen",
    "review the release candidate",
    "act",
    "needs_project",
    null,
    "2026-11-21T10:06:00",
  );
  return {
    notes,
    projects,
    tasks: [...visibleTasks, offscreenTask],
    settings: {
      importClickupEnabled: true,
      importGithubEnabled: true,
      importScope: "visual fixture",
      updatedAt: "2026-11-21T10:05:00",
    },
  };
};

const pickerRows: V2PickerRow[] = [
  {
    key: "card:CU-8x1k2",
    kind: "Card",
    displayId: "@CU-8x1k2",
    canonicalLink: "https://app.clickup.com/t/8x1k2",
    availability: "ready",
    group: "cards",
    title: "Retry storm on webhooks",
    state: "in progress",
    metadata: ["list · Platform", "assignee · you", "updated · 2h ago"],
    snapshot: null,
    unavailableReason: null,
    settingsDestination: null,
    updatedAt: "2026-11-21T10:05:00",
    exactMatch: false,
  },
  {
    key: "card:CU-7h2p9",
    kind: "Card",
    displayId: "@CU-7h2p9",
    canonicalLink: "https://app.clickup.com/t/7h2p9",
    availability: "ready",
    group: "cards",
    title: "CSV export times out",
    state: "open",
    metadata: ["list · Web", "assignee · unassigned", "updated · yesterday"],
    snapshot: null,
    unavailableReason: null,
    settingsDestination: null,
    updatedAt: "2026-11-20T10:05:00",
    exactMatch: false,
  },
  {
    key: "pr:billing-api#142",
    kind: "PR",
    displayId: "@billing-api#142",
    canonicalLink: "https://github.com/billing-api/pull/142",
    availability: "ready",
    group: "prs",
    title: "Backoff with jitter",
    state: "checks passing",
    metadata: ["repo · billing-api", "branch · fix/backoff", "+84 −12 · 3 files"],
    snapshot: null,
    unavailableReason: null,
    settingsDestination: null,
    updatedAt: "2026-11-21T10:05:00",
    exactMatch: false,
  },
  {
    key: "pr:billing-api#139",
    kind: "PR",
    displayId: "@billing-api#139",
    canonicalLink: "https://github.com/billing-api/pull/139",
    availability: "ready",
    group: "prs",
    title: "Add idempotency keys",
    state: "merged",
    metadata: ["repo · billing-api", "merged · Fri 20 Nov", "+210 −40 · 7 files"],
    snapshot: null,
    unavailableReason: null,
    settingsDestination: null,
    updatedAt: "2026-11-20T17:40:00",
    exactMatch: false,
  },
  {
    key: "pr:web-app#88",
    kind: "PR",
    displayId: "@web-app#88",
    canonicalLink: "https://github.com/web-app/pull/88",
    availability: "ready",
    group: "prs",
    title: "Settings layout pass",
    state: "review requested",
    metadata: ["repo · web-app", "branch · ui/settings", "+120 −98 · 5 files"],
    snapshot: null,
    unavailableReason: null,
    settingsDestination: null,
    updatedAt: "2026-11-20T12:00:00",
    exactMatch: false,
  },
  {
    key: "issue:web-app#91",
    kind: "Issue",
    displayId: "@web-app#91",
    canonicalLink: "https://github.com/web-app/issues/91",
    availability: "ready",
    group: "issues",
    title: "Dark mode contrast on tables",
    state: "open",
    metadata: ["repo · web-app", "labels · ui, a11y", "opened · 3d ago"],
    snapshot: null,
    unavailableReason: null,
    settingsDestination: null,
    updatedAt: "2026-11-18T12:00:00",
    exactMatch: false,
  },
  {
    key: "slack:#support/1731000000",
    kind: "Slack",
    displayId: "@#support/1731000000",
    canonicalLink: "https://slack.com/archives/support/p1731000000",
    availability: "ready",
    group: "conversations",
    title: "webhooks failing for some accounts",
    state: "12 replies",
    metadata: ["channel · #support", "last reply · 40m ago", "cached · 9:14"],
    snapshot: null,
    unavailableReason: null,
    settingsDestination: null,
    updatedAt: "2026-11-21T09:14:00",
    exactMatch: false,
  },
  {
    key: "note:N-12",
    kind: "Note",
    displayId: "@N-12",
    canonicalLink: "relay://note/N-12",
    availability: "ready",
    group: "notesTasks",
    title: "Standup: retry storm again",
    state: "note",
    metadata: ["written · today 09:12"],
    snapshot: null,
    unavailableReason: null,
    settingsDestination: null,
    updatedAt: "2026-11-21T09:12:00",
    exactMatch: false,
  },
];

export const visualFixturePickerRows = (query: string): V2PickerRow[] => {
  const needle = query.toLowerCase();
  return pickerRows.filter((row) => `${row.displayId} ${row.title}`.toLowerCase().includes(needle));
};

export const visualFixtureChips = (text: string): Chip[] =>
  pickerRows
    .filter((row) => text.includes(row.displayId))
    .map((row) => ({
      key: row.key,
      token: row.displayId,
      kind: row.kind.toLowerCase(),
      canonicalUrl: row.canonicalLink,
      originalUrl: row.displayId,
      unavailable: false,
    }));
