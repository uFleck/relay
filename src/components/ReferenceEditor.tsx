import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { lineDirective } from "../directives";
import {
  convertUrls,
  copyText,
  findMarks,
  isSlackAction,
  mergeLiveRows,
  mergeSlackHits,
  parseUrlValue,
  rowIndexByKey,
  slackActionRows,
  SLACK_REFRESH_KEY,
  SLACK_SEARCH_KEY,
  restoreChip,
  type Chip,
  type EditorState,
  type ProviderProgress,
  type V2PickerRow,
} from "../references";
import { useEntering } from "../ui/motion";
import {
  onReferenceChange,
  v2ReferenceExact,
  v2ReferenceParse,
  v2ReferencePicker,
  v2ReferencePick,
  v2ReferenceSearch,
  v2SlackRefresh,
  v2SlackSearch,
  v2SlackWindowStart,
} from "../ipc";
import { ReferencePicker } from "./ReferencePicker";

const LIVE_DEBOUNCE_MS = 250;
const SLOW_AFTER_MS = 3000;

export interface ReferenceEditorProps {
  value: string;
  onChange: (text: string) => void;
  placeholder?: string;
  ariaLabel?: string;
  autoFocus?: boolean;
  multiline?: boolean;
  onSubmit?: () => void;
  onActiveChange?: (active: boolean) => void;
  /** Forwarded by Composer so the Dock can hand its input to the native host. */
  inputRef?: RefObject<HTMLTextAreaElement | null>;
  /** Reports compact / picker / picker-with-Lens changes to the Dock. */
  onPickerChange?: (open: boolean, lensVisible: boolean) => void;
  onPickerTransitionEnd?: () => void;
  onEscConsumed?: () => void;
}

/* A textarea with a decoration overlay. The textarea keeps plain chip tokens so caret,
   selection, newlines and native editing stay predictable; the overlay paints them as
   inline chips. Local results appear on the first keystroke; live provider results arrive after
   two characters and a short debounce and never replace the local rows while pending. */
export function ReferenceEditor({
  value,
  onChange,
  placeholder,
  ariaLabel,
  autoFocus,
  multiline = true,
  onSubmit,
  onActiveChange,
  inputRef: forwardedRef,
  onPickerChange,
  onPickerTransitionEnd,
  onEscConsumed,
}: ReferenceEditorProps) {
  const [state, setState] = useState<EditorState>({ text: value, chips: [] });
  const [rows, setRows] = useState<V2PickerRow[]>([]);
  const [progress, setProgress] = useState<ProviderProgress[]>([]);
  const [slow, setSlow] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [highlight, setHighlight] = useState(0);
  const [errorToken, setErrorToken] = useState<string | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const decorationRef = useRef<HTMLDivElement>(null);
  const undo = useRef<EditorState[]>([]);
  const lastConversion = useRef<{ chip: Chip; delimiter: string; at: number } | null>(null);
  const lastPaste = useRef<{ at: number; previous: EditorState } | null>(null);
  const tokenRange = useRef<{ start: number; end: number } | null>(null);
  const debounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  const slowTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const queryRef = useRef("");
  const localRef = useRef<V2PickerRow[]>([]);
  const liveRef = useRef<V2PickerRow[]>([]);
  const rowsRef = useRef<V2PickerRow[]>([]);
  const highlightRef = useRef(0);
  /** One explicit search per trimmed query per picker session; typing alone never searches. */
  const searchedRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    if (value === state.text) return;
    setState({ text: value, chips: [] });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);

  // The Dock's input grows with its decoration so Rust can measure one stable content root.
  // Other hosts keep their own textarea sizing rules.
  useLayoutEffect(() => {
    const input = inputRef.current;
    const decoration = decorationRef.current;
    if (!input || !decoration) return;
    if (!input.closest(".dock")) {
      input.style.height = "";
      return;
    }
    const size = () => {
      input.style.height = `${decoration.offsetHeight}px`;
    };
    size();
    const observer = new ResizeObserver(size);
    observer.observe(decoration);
    return () => observer.disconnect();
  }, [state.text]);

  useEffect(() => onReferenceChange(() => {
    if (pickerOpen && queryRef.current) void runLocal(queryRef.current);
  }), [pickerOpen]);

  // The Dock owns the three native sizes; it only needs to know which one is on screen.
  useEffect(() => {
    onPickerChange?.(pickerOpen, pickerOpen && rows.length > 0);
  }, [pickerOpen, rows.length, onPickerChange]);

  // Entering identities drive the one-shot bead and chip animations. Plain keystrokes keep
  // both keys unchanged, so nothing replays while the user types.
  const lineKeys = state.text.split("\n").map((line, at) => `L${at}:${lineDirective(line) ?? "note"}`);
  const enteringLines = useEntering(lineKeys);
  const enteringChips = useEntering(state.chips.map((chip) => chip.key));

  const commit = (next: EditorState, caret?: number) => {
    setState(next);
    onChange(next.text);
    if (caret !== undefined) {
      requestAnimationFrame(() => {
        const input = inputRef.current;
        if (!input) return;
        input.selectionStart = caret;
        input.selectionEnd = caret;
      });
    }
  };

  /** Merge local and live rows without moving the highlighted key or the scroll position.
      Action rows keep stable ids, so a late search result never moves the highlight. */
  const applyRows = () => {
    const merged = mergeLiveRows(localRef.current, liveRef.current);
    const next = [...slackActionRows(queryRef.current), ...merged];
    const key = rowsRef.current[highlightRef.current]?.key;
    rowsRef.current = next;
    setRows(next);
    const nextHighlight = rowIndexByKey(next, key);
    highlightRef.current = nextHighlight;
    setHighlight(nextHighlight);
  };

  const moveHighlight = (next: number) => {
    highlightRef.current = next;
    setHighlight(next);
  };

  const clearTimers = () => {
    if (debounce.current) clearTimeout(debounce.current);
    if (slowTimer.current) clearTimeout(slowTimer.current);
    debounce.current = null;
    slowTimer.current = null;
  };

  const closePicker = () => {
    clearTimers();
    queryRef.current = "";
    localRef.current = [];
    liveRef.current = [];
    rowsRef.current = [];
    setRows([]);
    setProgress([]);
    setSlow(false);
    setPickerOpen(false);
    setErrorToken(null);
    tokenRange.current = null;
    searchedRef.current = new Set();
  };

  const runLocal = async (query: string) => {
    try {
      const found = await v2ReferencePicker(query);
      if (queryRef.current !== query) return;
      localRef.current = found;
      applyRows();
      setPickerOpen(found.length > 0 || query.length >= 2);
      setErrorToken(found.length === 0 && query.length >= 2 ? `@${query}` : null);
    } catch {
      // A missing command must not quietly look like an empty picker.
      if (queryRef.current === query) closePicker();
    }
  };

  const runLive = async (query: string) => {
    const optimistic: ProviderProgress = query.toLowerCase().startsWith("cu-")
      ? { provider: "clickup", state: "searching", reason: null, settingsDestination: null }
      : { provider: "github", state: "searching", reason: null, settingsDestination: null };
    setProgress([optimistic]);
    setSlow(false);
    if (slowTimer.current) clearTimeout(slowTimer.current);
    slowTimer.current = setTimeout(() => {
      if (queryRef.current === query) setSlow(true);
    }, SLOW_AFTER_MS);
    try {
      const result = await v2ReferenceSearch(query);
      // An old response is ignored after the query changed or the picker closed. A response
      // for the current query is accepted however late it arrives, and can retry on a new query.
      if (queryRef.current !== query) return;
      liveRef.current = result.rows;
      setProgress(result.progress);
      applyRows();
    } catch {
      if (queryRef.current === query) setProgress([]);
    }
  };

  /** Manual Refresh conversations. A quiet spinner stays up until the backend settles; the
      single-flight guard in core makes a second press harmless. */
  const refreshConversations = async () => {
    setProgress([{ provider: "slack", state: "searching", reason: null, settingsDestination: null }]);
    setSlow(false);
    try {
      const report = await v2SlackRefresh();
      if (queryRef.current) void runLocal(queryRef.current);
      setProgress(
        report.error
          ? [{ provider: "slack", state: "unavailable", reason: report.error, settingsDestination: "Settings → Slack" }]
          : [],
      );
    } catch {
      setProgress([]);
    }
  };

  /** One Enter-driven detailed search per query per session. Typing never reaches this path. */
  const runSlackSearch = async (query: string) => {
    if (searchedRef.current.has(query)) return;
    searchedRef.current.add(query);
    setProgress([{ provider: "slack", state: "searching", reason: null, settingsDestination: null }]);
    setSlow(false);
    try {
      const result = await v2SlackSearch(query);
      // A response for a closed picker or a different active query is discarded, however late.
      if (queryRef.current !== query) return;
      liveRef.current = mergeSlackHits(liveRef.current, result.hits);
      applyRows();
      setProgress(
        result.error
          ? [{ provider: "slack", state: "unavailable", reason: result.error, settingsDestination: "Settings → Slack" }]
          : [],
      );
    } catch {
      if (queryRef.current === query) setProgress([]);
    }
  };

  const trackTypedToken = (text: string, caret: number) => {
    let start = caret;
    while (start > 0 && !/\s/.test(text[start - 1])) start -= 1;
    const token = text.slice(start, caret);
    clearTimers();
    if (!token.startsWith("@") || token.length < 2) {
      closePicker();
      return;
    }
    const bare = token.slice(1);
    tokenRange.current = { start, end: caret };
    queryRef.current = bare;
    liveRef.current = [];
    localRef.current = [];
    applyRows();
    // Local rows are usable from the first keystroke.
    void runLocal(bare);
    if (bare.length >= 2) {
      debounce.current = setTimeout(() => void runLive(bare), LIVE_DEBOUNCE_MS);
    }
  };

  const changed = (text: string, caret: number) => {
    lastConversion.current = null;
    lastPaste.current = null;
    const chips = state.chips.filter((chip) => text.includes(chip.token));
    commit({ text, chips });
    trackTypedToken(text, caret);
  };

  const insertChip = async (row: V2PickerRow) => {
    const range = tokenRange.current;
    if (!range) return;
    let accepted = row;
    let pending: V2PickerRow | undefined;
    if (row.kind === "slackchannel") {
      // A picked conversation is a fresh frozen window: the id is created before the read starts,
      // so two picks of the same channel survive reopen and edit as distinct References.
      const channelId = row.key.slice("slackconv:".length);
      let host: string | null = null;
      try {
        host = new URL(row.canonicalLink).host;
      } catch {
        host = null;
      }
      const name = row.title && row.title !== channelId ? row.title : null;
      const started = await v2SlackWindowStart(channelId, host, name);
      if (!started) return;
      accepted = started;
      pending = started;
    }
    undo.current.push(state);
    lastConversion.current = null;
    lastPaste.current = null;
    const existing = state.chips.find((chip) => chip.key === accepted.key);
    const chip: Chip = existing ?? {
      key: accepted.key,
      token: accepted.displayId,
      kind: accepted.kind,
      canonicalUrl: accepted.canonicalLink,
      originalUrl: accepted.displayId,
      unavailable: accepted.availability === "unavailable",
    };
    const before = state.text.slice(0, range.start);
    const after = state.text.slice(range.end);
    const next = {
      text: before + chip.token + after,
      chips: existing ? state.chips : [...state.chips, chip],
    };
    commit(next, before.length + chip.token.length);
    // Persist the accepted result now; this writes only a snapshot, never a Note or Task.
    if (accepted.kind !== "slackwindow") void v2ReferencePick(accepted).catch(() => undefined);
    closePicker();
    if (accepted.kind === "slack" || accepted.kind === "slackwindow") {
      startSlackFetch(chip, pending);
    }
  };

  /** Starts a deduplicated exact read for a known pasted or linked URL while the draft is open. */
  const exactRead = (chip: Chip) => {
    void v2ReferenceExact(chip.key, chip.canonicalUrl)
      .then((row) => {
        if (!row || !pickerOpen || queryRef.current) return;
        liveRef.current = mergeLiveRows(liveRef.current, [row]);
        applyRows();
      })
      .catch(() => undefined);
  };

  /** A Slack permalink is a thread read, not a work-provider exact read. Keep the Lens open with
      a linked "fetching…" row, then replace that row in place from the shared snapshot path. */
  const startSlackFetch = (chip: Chip, started?: V2PickerRow) => {
    const pending: V2PickerRow = started ?? {
      key: chip.key,
      kind: "slack",
      displayId: chip.token,
      canonicalLink: chip.canonicalUrl,
      availability: "ready",
      group: "conversations",
      title: "fetching…",
      state: null,
      metadata: [],
      snapshot: {
        title: "fetching…",
        state: null,
        metadata: [],
        cachedAt: null,
        reason: null,
        content: null,
        channelName: null,
      },
      unavailableReason: null,
      settingsDestination: null,
      updatedAt: new Date().toISOString(),
      exactMatch: true,
    };
    clearTimers();
    queryRef.current = "";
    liveRef.current = [pending];
    applyRows();
    highlightRef.current = 0;
    setHighlight(0);
    setPickerOpen(true);
    setProgress([
      { provider: "slack", state: "searching", reason: null, settingsDestination: null },
    ]);
    void v2ReferenceExact(chip.key, chip.canonicalUrl)
      .then((row) => {
        const resolved = row ?? {
          ...pending,
          availability: "unavailable" as const,
          title: "",
          unavailableReason: "Slack thread is currently unreadable",
          snapshot: {
            ...pending.snapshot!,
            title: null,
            reason: "Slack thread is currently unreadable",
          },
        };
        liveRef.current = mergeLiveRows(liveRef.current, [resolved]);
        applyRows();
        setProgress([]);
      })
      .catch(() => {
        liveRef.current = mergeLiveRows(liveRef.current, [
          {
            ...pending,
            availability: "unavailable",
            title: "",
            unavailableReason: "Slack thread is currently unreadable",
          },
        ]);
        applyRows();
        setProgress([]);
      });
  };

  const paste = async (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const text = event.clipboardData.getData("text/plain");
    if (!text.includes("https://")) return;
    event.preventDefault();
    try {
      const parsed = await v2ReferenceParse(text);
      if (parsed.urls.length === 0) {
        throw new Error("no recognised URLs");
      }
      const input = event.currentTarget;
      const start = input.selectionStart;
      const end = input.selectionEnd;
      const combined = state.text.slice(0, start) + text + state.text.slice(end);
      const shifted = parsed.urls.map((match) => ({
        ...match,
        start: match.start + start,
        end: match.end + start,
      }));
      undo.current.push(state);
      const converted = convertUrls(state, combined, shifted);
      const before = state.text.slice(0, start);
      const after = state.text.slice(end);
      const caret = start + (converted.text.length - before.length - after.length);
      lastPaste.current = { at: caret, previous: state };
      commit(converted, caret);
      closePicker();
      for (const match of parsed.urls) {
        const chip = converted.chips.find((candidate) => candidate.key === match.key);
        if (!chip) continue;
        if (chip.kind === "slack") startSlackFetch(chip);
        else exactRead(chip);
      }
    } catch {
      event.preventDefault();
    }
  };

  const convertTypedUrl = async (delimiter: string) => {
    const input = inputRef.current;
    if (!input) return false;
    const caret = input.selectionStart;
    let start = caret;
    while (start > 0 && !/\s/.test(state.text[start - 1])) start -= 1;
    const token = state.text.slice(start, caret);
    if (!token.startsWith("https://")) return false;
    const parsed = parseUrlValue(token);
    if (!parsed) return false;
    const chip: Chip = {
      key: parsed.identity,
      token: parsed.display,
      kind: parsed.kind,
      canonicalUrl: parsed.canonical,
      originalUrl: token,
    };
    undo.current.push(state);
    lastPaste.current = null;
    const before = state.text.slice(0, start);
    const after = state.text.slice(caret);
    const next = {
      text: before + chip.token + delimiter + after,
      chips: [...state.chips.filter((item) => item.key !== chip.key), chip],
    };
    commit(next, before.length + chip.token.length + delimiter.length);
    lastConversion.current = { chip, delimiter, at: before.length + chip.token.length + delimiter.length };
    closePicker();
    if (chip.kind === "slack") startSlackFetch(chip);
    else exactRead(chip);
    return true;
  };

  const keyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
      const previous = undo.current.pop();
      if (previous) {
        event.preventDefault();
        commit(previous);
        closePicker();
      }
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      onSubmit?.();
      return;
    }
    if (pickerOpen) {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onEscConsumed?.();
        closePicker();
        return;
      }
      if (event.key === "ArrowDown") {
        event.preventDefault();
        moveHighlight(rows.length ? (highlightRef.current + 1) % rows.length : 0);
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        moveHighlight(rows.length ? (highlightRef.current - 1 + rows.length) % rows.length : 0);
        return;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        const row = rows[highlightRef.current];
        if (!row) return;
        if (isSlackAction(row)) {
          // Only Enter activates an action; a dismissed Tab leaves the draft untouched.
          if (event.key === "Enter") {
            if (row.key === SLACK_REFRESH_KEY) void refreshConversations();
            else if (row.key === SLACK_SEARCH_KEY) void runSlackSearch(queryRef.current.trim());
          }
          return;
        }
        void insertChip(row);
        return;
      }
    }
    if (event.key === "Backspace") {
      const caret = event.currentTarget.selectionStart;
      const conversion = lastConversion.current;
      if (conversion && caret === conversion.at) {
        event.preventDefault();
        undo.current.push(state);
        commit(
          restoreChip(state, conversion.chip, conversion.delimiter),
          conversion.at - conversion.delimiter.length,
        );
        lastConversion.current = null;
        return;
      }
      const pasted = lastPaste.current;
      if (pasted && caret === pasted.at) {
        event.preventDefault();
        commit(pasted.previous, pasted.at);
        lastPaste.current = null;
      }
      return;
    }
    if (event.key === " " || event.key === "Enter") {
      const caret = event.currentTarget.selectionStart;
      let start = caret;
      while (start > 0 && !/\s/.test(state.text[start - 1])) start -= 1;
      const token = state.text.slice(start, caret);
      if (token.startsWith("https://") && parseUrlValue(token)) {
        event.preventDefault();
        void convertTypedUrl(event.key === " " ? " " : "\n");
      }
      // A plain Space or Enter keeps its native behavior and delimiter.
    }
  };

  const copy = (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
    if (state.chips.length === 0) return;
    const input = event.currentTarget;
    event.preventDefault();
    if (input.selectionStart === input.selectionEnd) {
      event.clipboardData.setData("text/plain", copyText(state));
      return;
    }
    const selected = state.text.slice(input.selectionStart, input.selectionEnd);
    const rendered = selected.replace(/@[^\s]+/g, (token) => {
      const chip = state.chips.find((candidate) => candidate.token === token);
      return chip ? chip.canonicalUrl : token;
    });
    event.clipboardData.setData("text/plain", rendered);
  };

  /** One block per draft line. The bead hangs off the block, so it stays on the first
      visual row even when a long line wraps; the textarea's own text is never touched. */
  const decorate = () => {
    const lines = state.text.split("\n");
    const starts: number[] = [];
    let from = 0;
    for (const line of lines) {
      starts.push(from);
      from += line.length + 1;
    }
    const marks = findMarks(state, errorToken);
    return lines.map((line, at) => {
      const mode = lineDirective(line);
      const base = starts[at];
      const end = base + line.length;
      const nodes: React.ReactNode[] = [];
      let cursor = base;
      for (const mark of marks) {
        if (mark.at < base || mark.at >= end) continue;
        nodes.push(state.text.slice(cursor, mark.at));
        const chip = mark.kind === "chip" ? state.chips.find((item) => item.key === mark.key) : null;
        const pop = enteringChips.has(mark.key) ? " pop" : "";
        nodes.push(
          <span
            className={`${mark.kind === "chip" ? "ref-chip" : "ref-token-error"}${pop}`}
            data-unresolved={chip?.unavailable ? "true" : undefined}
            key={`${mark.key}-${mark.at}`}
          >
            {mark.text}
          </span>,
        );
        cursor = mark.at + mark.text.length;
      }
      nodes.push(state.text.slice(cursor, end));
      const bead = mode && enteringLines.has(lineKeys[at]) ? " pop" : "";
      return (
        <span className={`de-line${bead}`} data-mode={mode ?? undefined} key={lineKeys[at]}>
          {nodes.length > 0 ? nodes : "\u200b"}
        </span>
      );
    });
  };

  return (
    <div className="ref-editor">
      <div className="ref-editor-surface">
        <div className="ref-editor-decoration" aria-hidden="true" ref={decorationRef}>
          {decorate()}
        </div>
        <textarea
          ref={(node) => {
            (inputRef as { current: HTMLTextAreaElement | null }).current = node;
            if (forwardedRef) {
              (forwardedRef as { current: HTMLTextAreaElement | null }).current = node;
            }
          }}
          className={`ref-editor-input${multiline ? "" : " single"}`}
          value={state.text}
          placeholder={placeholder}
          aria-label={ariaLabel ?? "Composer"}
          autoFocus={autoFocus}
          onChange={(event) => changed(event.target.value, event.target.selectionStart)}
          onKeyDown={keyDown}
          onPaste={(event) => void paste(event)}
          onCopy={copy}
          onFocus={() => onActiveChange?.(true)}
          onBlur={() => onActiveChange?.(false)}
        />
      </div>
      <ReferencePicker
        rows={rows}
        highlight={highlight}
        onHighlight={moveHighlight}
        onPick={insertChip}
        onClose={closePicker}
        progress={progress}
        slow={slow}
        open={pickerOpen}
        onTransitionEnd={onPickerTransitionEnd}
      />
    </div>
  );
}
