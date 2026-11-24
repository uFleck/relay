/* Shared Reference contracts and pure chip logic. The Rust commands own canonical parsing
   in the app; this module mirrors it for browser development and gives the editor pure,
   testable transforms for paste, copy, undo and backspace. */

export type ReferenceAvailability = "ready" | "unavailable";
export type V2PickerGroup = "cards" | "prs" | "issues" | "conversations" | "notesTasks";

export const GROUP_LABEL: Record<V2PickerGroup, string> = {
  cards: "Cards",
  prs: "PR",
  issues: "Issues",
  conversations: "Conversations",
  notesTasks: "Notes / Tasks",
};

export const GROUP_ORDER: V2PickerGroup[] = [
  "cards",
  "prs",
  "issues",
  "conversations",
  "notesTasks",
];

/** Stable action-row ids. `slack-search` never moves, so highlighting survives search results. */
export const SLACK_REFRESH_KEY = "action:slack-refresh";
export const SLACK_SEARCH_KEY = "action:slack-search";

/** One detailed `slack_search_public_and_private` hit, as returned by the Rust boundary. */
export interface SlackSearchHit {
  channelId: string;
  channelName: string;
  ts: string;
  permalink: string;
  threadTs: string | null;
  snippet: string;
}

function actionRow(key: string, displayId: string, title: string): V2PickerRow {
  return {
    key,
    kind: "action",
    displayId,
    canonicalLink: "",
    availability: "ready",
    group: "conversations",
    title,
    state: null,
    metadata: [],
    snapshot: null,
    unavailableReason: null,
    settingsDestination: null,
    updatedAt: "1970-01-01T00:00:00.000Z",
    exactMatch: false,
  };
}

/** Conversations always offer Refresh; a nonempty query also gets one Enter-driven search row. */
export function slackActionRows(query: string): V2PickerRow[] {
  const rows = [actionRow(SLACK_REFRESH_KEY, "↻", "Refresh conversations")];
  const trimmed = query.trim();
  if (trimmed.length > 0) {
    rows.push(actionRow(SLACK_SEARCH_KEY, "⌕", `Search Slack messages for “${trimmed}”`));
  }
  return rows;
}

export function isSlackAction(row: V2PickerRow | undefined): boolean {
  return !!row && (row.key === SLACK_REFRESH_KEY || row.key === SLACK_SEARCH_KEY);
}

/** A search hit is a thread Reference: the permalink carries the root `thread_ts` for a reply. */
export function slackHitRow(hit: SlackSearchHit): V2PickerRow | null {
  const parsed = parseUrlValue(hit.permalink);
  if (!parsed) return null;
  const firstLine = hit.snippet.split("\n")[0].trim();
  return {
    key: parsed.identity,
    kind: parsed.kind,
    displayId: parsed.display,
    canonicalLink: parsed.canonical,
    availability: "ready",
    group: "conversations",
    title: firstLine || "Slack message",
    state: null,
    metadata: [hit.channelName || hit.channelId],
    snapshot: null,
    unavailableReason: null,
    settingsDestination: null,
    updatedAt: new Date().toISOString(),
    exactMatch: false,
  };
}

/** Deduplicates by canonical thread key, refreshing matching rows in place and appending hits. */
export function mergeSlackHits(rows: V2PickerRow[], hits: SlackSearchHit[]): V2PickerRow[] {
  return mergeLiveRows(
    rows,
    hits.map(slackHitRow).filter((row): row is V2PickerRow => row !== null),
  );
}

export interface ReferenceSnapshot {
  title: string | null;
  state: string | null;
  metadata: string[];
  cachedAt: string | null;
  reason: string | null;
  content: string | null;
  channelName: string | null;
}

export interface V2PickerRow {
  key: string;
  kind: string;
  displayId: string;
  canonicalLink: string;
  availability: ReferenceAvailability;
  group: V2PickerGroup;
  title: string;
  state: string | null;
  metadata: string[];
  snapshot: ReferenceSnapshot | null;
  unavailableReason: string | null;
  settingsDestination: string | null;
  updatedAt: string;
  exactMatch: boolean;
}

export type ProviderProgressState = "searching" | "ok" | "unavailable";

export interface ProviderProgress {
  provider: string;
  state: ProviderProgressState;
  reason: string | null;
  settingsDestination: string | null;
}

export interface LiveQueryResult {
  query: string;
  rows: V2PickerRow[];
  progress: ProviderProgress[];
}

/** Refreshes duplicates in place and appends new rows. Group order is applied later by
    `groupRows`, so an appended row lands at the end of its own kind group. */
export function mergeLiveRows(local: V2PickerRow[], live: V2PickerRow[]): V2PickerRow[] {
  const out = local.slice();
  const index = new Map(out.map((row, at) => [row.key, at]));
  for (const row of live) {
    const at = index.get(row.key);
    if (at === undefined) {
      index.set(row.key, out.length);
      out.push(row);
    } else {
      out[at] = row;
    }
  }
  return out;
}

/** The index a highlighted key moves to after rows are merged. Missing keys fall back to the
    first row, so a late live response never leaves the highlight pointing past the list. */
export function rowIndexByKey(rows: V2PickerRow[], key: string | undefined): number {
  if (!key) return 0;
  const at = rows.findIndex((row) => row.key === key);
  return at >= 0 ? at : 0;
}

export interface UrlMatch {
  start: number;
  end: number;
  value: string;
  key: string;
  kind: string;
  canonicalLink: string;
  displayId: string;
}

export interface ReferenceParseResult {
  urls: UrlMatch[];
}

/** A chip is a display token in the textarea plus the URL it stands for. Identity is the
    key; title, availability and metadata are read from the picker row and never change it. */
export interface Chip {
  key: string;
  token: string;
  kind: string;
  canonicalUrl: string;
  originalUrl: string;
  /** A picked target that is linked but currently unreadable paints as an unresolved Reference. */
  unavailable?: boolean;
}

export interface EditorState {
  text: string;
  chips: Chip[];
}

const HOSTS = /^(slack\.com|[a-z0-9-]+\.slack\.com)$/i;

function normalizeTs(raw: string): string | null {
  if (!raw) return null;
  if (raw.includes(".")) {
    const [seconds, micros] = raw.split(".");
    if (!/^\d+$/.test(seconds) || !/^\d+$/.test(micros)) return null;
    return raw;
  }
  if (!/^\d+$/.test(raw)) return null;
  return raw.length > 10
    ? `${raw.slice(0, raw.length - 6)}.${raw.slice(raw.length - 6)}`
    : raw;
}

interface Parsed {
  identity: string;
  kind: string;
  canonical: string;
  display: string;
}

/** Canonical URL recognition. Mirrors `relay-core::v2::reference::parse_urls`. */
export function parseUrlValue(raw: string): Parsed | null {
  const [basePart, query] = raw.split("?");
  const base = basePart.split("#")[0];

  const github = base.match(/^https:\/\/github\.com\/([^/]+)\/([^/]+)\/(pull|issues)\/(\d+)\/?$/i);
  if (github) {
    const owner = github[1].toLowerCase();
    const repo = github[2].toLowerCase();
    const number = Number(github[4]);
    if (!number) return null;
    const item = github[3].toLowerCase();
    return {
      identity: `github:${owner}/${repo}#${number}`,
      kind: "github",
      canonical: `https://github.com/${owner}/${repo}/${item}/${number}`,
      display: `@${repo}#${number}`,
    };
  }

  const clickup = base.match(/^https:\/\/app\.clickup\.com\/t\/([^/]+)\/?$/i);
  if (clickup && clickup[1]) {
    const id = clickup[1];
    return {
      identity: `clickup:${id}`,
      kind: "clickup",
      canonical: `https://app.clickup.com/t/${id}`,
      display: `@CU-${id}`,
    };
  }

  if (base.startsWith("https://")) {
    const rest = base.slice(8);
    const slash = rest.indexOf("/");
    if (slash > 0) {
      const host = rest.slice(0, slash);
      if (HOSTS.test(host)) {
        const parts = rest.slice(slash + 1).replace(/\/$/, "").split("/");
        if (parts[0] === "archives" && parts[1] && parts[2]?.startsWith("p")) {
          const permalinkTs = normalizeTs(parts[2].slice(1));
          const thread = query
            ?.split("&")
            .find((pair) => pair.startsWith("thread_ts="))
            ?.slice("thread_ts=".length);
          const parent = (thread && normalizeTs(thread)) || permalinkTs;
          if (parent) {
            return {
              identity: `slack:${parts[1]}/${parent}`,
              kind: "slack",
              // Keep the pasted message (a reply keeps its own timestamp) for copy and
              // navigation; the root travels as thread_ts and drives identity and caching.
              canonical: `https://${host}/archives/${parts[1]}/p${permalinkTs!.replace(".", "")}?thread_ts=${parent}`,
              display: `@${parts[1]}/${parent.split(".")[0]}`,
            };
          }
        }
      }
    }
  }
  return null;
}

/** Scan free text for recognised URLs. Offsets are character offsets. */
export function parseUrls(text: string): ReferenceParseResult {
  const urls: UrlMatch[] = [];
  let from = 0;
  while (true) {
    const at = text.indexOf("https://", from);
    if (at < 0) break;
    let end = text.length;
    for (let i = at; i < text.length; i += 1) {
      const ch = text[i];
      if (/\s/.test(ch) || ch === "<" || ch === ">" || ch === '"' || ch === "'" || ch === "`") {
        end = i;
        break;
      }
    }
    let value = text.slice(at, end);
    value = value.replace(/[.,;:!?)\]}"']+$/, "");
    const parsed = value && parseUrlValue(value);
    if (parsed) {
      urls.push({
        start: at,
        end: at + value.length,
        value,
        key: parsed.identity,
        kind: parsed.kind,
        canonicalLink: parsed.canonical,
        displayId: parsed.display,
      });
    }
    from = at + 8;
  }
  return { urls };
}

/** Replace recognised URL spans with chip tokens in one pass, preserving every other
    character (including newlines and punctuation). Existing chips are carried forward by
    token so the identity and caret stay stable. */
export function convertUrls(state: EditorState, text: string, matches: UrlMatch[]): EditorState {
  const chips = state.chips.slice();
  let output = "";
  let cursor = 0;
  for (const match of matches) {
    output += text.slice(cursor, match.start);
    const existing = chips.find((chip) => chip.key === match.key);
    const token = existing?.token ?? match.displayId;
    if (!existing) {
      chips.push({
        key: match.key,
        token,
        kind: match.kind,
        canonicalUrl: match.canonicalLink,
        originalUrl: match.value,
      });
    }
    output += token;
    cursor = match.end;
  }
  output += text.slice(cursor);
  return { text: output, chips };
}

/** Copy renders URL-backed chips as their canonical plain-text links. */
export function copyText(state: EditorState): string {
  return state.text.replace(/@[^\s]+/g, (token) => {
    const chip = state.chips.find((candidate) => candidate.token === token);
    return chip ? chip.canonicalUrl : token;
  });
}

/** The chip token that ends exactly at `caret`, if any. */
export function chipAtCaret(state: EditorState, caret: number): Chip | null {
  for (const chip of state.chips) {
    const at = state.text.lastIndexOf(chip.token, Math.max(0, caret));
    if (at >= 0 && at + chip.token.length === caret) return chip;
  }
  return null;
}

/** Restore one chip to its original plain URL. `delimiter` is the Space or Enter that
    triggered a typed conversion and is removed too, so Backspace returns the exact text. */
export function restoreChip(state: EditorState, chip: Chip, delimiter: string): EditorState {
  const at = state.text.indexOf(chip.token);
  if (at < 0) return state;
  const end = at + chip.token.length + (state.text.startsWith(delimiter, at + chip.token.length) ? delimiter.length : 0);
  return {
    text: state.text.slice(0, at) + chip.originalUrl + state.text.slice(end),
    chips: state.chips.filter((candidate) => candidate.key !== chip.key),
  };
}

export interface TextMark {
  at: number;
  text: string;
  kind: "chip" | "error";
  key: string;
}

/** Find decoration ranges by scanning forward, so a repeated chip token on two lines gets
    one mark per occurrence at its own offset instead of both pointing at indexOf's first hit. */
export function findMarks(state: EditorState, errorToken: string | null): TextMark[] {
  const patterns = [
    ...state.chips.map((chip) => ({ text: chip.token, kind: "chip" as const, key: chip.key })),
    ...(errorToken
      ? [{ text: errorToken, kind: "error" as const, key: "token-error" }]
      : []),
  ].filter((pattern) => pattern.text.length > 0);
  const marks: TextMark[] = [];
  let cursor = 0;
  while (cursor < state.text.length) {
    let next: TextMark | null = null;
    for (const pattern of patterns) {
      const at = state.text.indexOf(pattern.text, cursor);
      if (at >= 0 && (next === null || at < next.at)) {
        next = { at, ...pattern };
      }
    }
    if (!next) break;
    marks.push(next);
    cursor = next.at + next.text.length;
  }

  return markUnresolvedTokens(state, marks);
}

function markUnresolvedTokens(state: EditorState, marks: TextMark[]) {
  for (const match of state.text.matchAll(/@[\w#/.-]+/g)) {
    const at = match.index ?? 0;
    if (marks.some((mark) => at >= mark.at && at < mark.at + mark.text.length)) continue;
    marks.push({ at, text: match[0], kind: "error", key: `unresolved:${at}` });
  }
  return marks.sort((a, b) => a.at - b.at);
}

/** Group picker rows in the fixed order, omitting empty groups. */
export function groupRows(rows: V2PickerRow[]): Array<{ group: V2PickerGroup; rows: V2PickerRow[] }> {
  return GROUP_ORDER.map((group) => ({
    group,
    rows: rows.filter((row) => row.group === group),
  })).filter((section) => section.rows.length > 0);
}

/** Human cache age for the Lens, from an RFC 3339 timestamp. */
export function cacheAge(cachedAt: string | null, now = Date.now()): string | null {
  if (!cachedAt) return null;
  const then = Date.parse(cachedAt);
  if (Number.isNaN(then)) return null;
  const minutes = Math.max(0, Math.round((now - then) / 60000));
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}
