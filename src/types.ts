/* View model shared by the main window and the hover. Rust owns the real data; these shapes are what the UI reads. */

export type StatusCategory =
  | "todo"
  | "active"
  | "review"
  | "merged"
  | "waiting"
  | "deploying"
  | "blocked"
  | "done";

export interface Status {
  id: string;
  name: string;
  /** Short label for the sheet stepper. */
  short: string;
  category: StatusCategory;
}

export interface Project {
  id: string;
  name: string;
  /** Circuit identity token, e.g. "--i1". */
  colorVar: string;
  /** The linked checkout. Optional: snapshots that predate the field leave it out. */
  repoPath?: string | null;
}

export type ClaimHolder = "T3" | "OpenCode" | "Claude Code";

export interface Claim {
  id: string;
  holder: ClaimHolder;
  claimedAt: string;
  lastProgressAt: string;
  lastProgress: string;
  leaseMinutes: number;
  releasedAt: string | null;
}

export type Source = "slack" | "github" | "clickup" | "agent";

export type Change =
  | {
      id: string;
      kind: "requirement";
      at: string;
      source: Source;
      author: string;
      text: string;
      confidence: number;
      permalink: string;
      ackedAt: string | null;
      ackedBy: string | null;
    }
  | {
      id: string;
      kind: "field";
      at: string;
      source: Source;
      field: string;
      from: string;
      to: string;
      ref: string;
    };

export type LinkKind = "slack" | "thread" | "pr" | "branch" | "clickup" | "github";

export interface TaskLink {
  kind: LinkKind;
  label: string;
  state: string;
  url: string;
  /** PR links: `{ checks, review, headSha }` once the poller has synced them. */
  metadataJson?: string;
}

export interface Due {
  at: string;
  hasTime: boolean;
}

export interface Task {
  id: string;
  ref: string;
  rev: number;
  title: string;
  projectId: string;
  statusId: string;
  due: Due | null;
  claim: Claim | null;
  description: string;
  changes: Change[];
  links: TaskLink[];
}

export interface ConvoPart {
  text: string;
  match?: boolean;
}

export interface ConvoMessage {
  at: string;
  who: string;
  me?: boolean;
  parts: ConvoPart[];
}

export interface JevScore {
  label: string;
  value: number;
  threshold: number;
  tone: "accent" | "muted" | "warn";
}

export interface FieldDiff {
  field: string;
  from: string;
  to: string;
}

export interface Proposal {
  id: string;
  kind: "change_deadline" | "add_requirement" | "new_task";
  taskId: string;
  taskRev: number;
  origin: string;
  author: string;
  episode: number;
  messages: ConvoMessage[];
  jev: { stage: string; ms: number; scores: JevScore[] };
  diff: FieldDiff[];
  /** What approving writes to the task; the UI applies it optimistically. */
  patch: { due?: Due; requirement?: string };
  note: string;
}

export type ProposalDecision = "approve" | "reject";

export type SyncState = "ok" | "retrying" | "down" | "off";

export interface SyncSource {
  name: string;
  short: string;
  state: SyncState;
  reason: string | null;
}

export type UpdateState =
  | { state: "idle"; version: string; checkedAt: string }
  | { state: "ready"; version: string; sizeMb: number; signed: boolean; note: string };

export interface Snapshot {
  version: string;
  projects: Project[];
  statuses: Status[];
  tasks: Task[];
  proposals: Proposal[];
  sync: SyncSource[];
  update: UpdateState;
  inboxCount: number;
  lastProposalHandledAt: string | null;
}

export type HoverMode = "tab" | "open";
export type HoverEdge = "left" | "right";
