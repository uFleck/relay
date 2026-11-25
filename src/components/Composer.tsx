import { useEffect, useMemo, useRef, useState } from "react";
import { draftLabel, draftSummary, type DraftSummary } from "../directives";
import { useDock } from "../hover/dock";
import {
  errorKind,
  errorText,
  v2ComposerSave,
  type V2ComposerSaveRequest,
  type V2LineInput,
  type V2Note,
} from "../ipc";
import { ReferenceEditor } from "./ReferenceEditor";

/* One input for both surfaces. Create sends text; Edit sends fields keyed by the saved
   lineId, so a promoted Task keeps its source line even after the text is rewritten. */

// Keep stable ids for unchanged lines, including moves within a replaced section.
// Reuse the remaining ids in order for lines whose text was rewritten.
function reviseLines(current: V2LineInput[], text: string): V2LineInput[] {
  const next = text.split("\n");
  let start = 0;
  while (start < next.length && current[start]?.text === next[start]) start++;
  let oldEnd = current.length;
  let newEnd = next.length;
  while (oldEnd > start && newEnd > start && current[oldEnd - 1].text === next[newEnd - 1]) {
    oldEnd--;
    newEnd--;
  }
  const changed = current.slice(start, oldEnd);
  const indexesByText = new Map<string, number[]>();
  for (let index = changed.length - 1; index >= 0; index--) {
    const { text } = changed[index];
    const indexes = indexesByText.get(text) ?? [];
    indexes.push(index);
    indexesByText.set(text, indexes);
  }
  const middle = next.slice(start, newEnd);
  const matches = middle.map((text) => indexesByText.get(text)?.pop());
  const used = new Set(matches);
  const rewritten = changed.filter((_, index) => !used.has(index)).values();
  const lines = middle.map((text, index) => {
    const previous = matches[index];
    if (previous !== undefined) return changed[previous];
    return { lineId: rewritten.next().value?.lineId ?? null, text };
  });
  return [...current.slice(0, start), ...lines, ...current.slice(oldEnd)];
}

export interface ComposerProps {
  /** Present means Edit mode: lines carry their saved ids and the save is revision-guarded. */
  note?: V2Note;
  onSaved?: (note: V2Note) => void;
  onNotice?: (message: string) => void;
  /** The hover keeps its panel open while this reports active (focused or saving). */
  onActiveChange?: (active: boolean) => void;
  /** Shown only after a stale-revision failure; the parent reloads the stored Note. */
  onReload?: () => void;
  compact?: boolean;
  /** The Dock: a draft that starts with a note or task reference is appended to that note. */
  capture?: boolean;
  autoFocus?: boolean;
  onClose?: () => void;
  /** Reports the draft's first mode and counts to the host's wordmark and footer. */
  onDraftChange?: (draft: DraftSummary) => void;
}

const newKey = () =>
  typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random()}`;

export function Composer({
  note,
  onSaved,
  onNotice,
  onActiveChange,
  onReload,
  compact,
  capture,
  autoFocus,
  onClose,
  onDraftChange,
}: ComposerProps) {
  const dock = useDock();
  const [text, setText] = useState("");
  const [lines, setLines] = useState<V2LineInput[]>(() =>
    (note?.lines ?? []).map((line) => ({ lineId: line.id, text: line.text })),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [stale, setStale] = useState(false);
  const [focused, setFocused] = useState(false);
  const busyRef = useRef(false);
  const localInput = useRef<HTMLTextAreaElement>(null);
  const inputRef = dock?.inputRef ?? localInput;
  const submissionKey = useRef<string | null>(null);

  const draftText = note ? lines.map((line) => line.text).join("\n") : text;
  const draft = useMemo(() => draftSummary(draftText), [draftText]);

  useEffect(() => {
    if (autoFocus) inputRef.current?.focus();
  }, [autoFocus, inputRef]);

  useEffect(() => {
    onActiveChange?.(focused || busy);
  }, [focused, busy, onActiveChange]);

  useEffect(() => {
    onDraftChange?.(draft);
    dock?.reportDraft(draft);
  }, [draft, onDraftChange, dock?.reportDraft]);

  const reportPicker = useMemo(
    () => (open: boolean, lensVisible: boolean) => {
      // Only the create-mode Composer drives the Dock's native size.
      if (!note) dock?.reportPicker(open, lensVisible);
    },
    [note, dock?.reportPicker],
  );

  const changeText = (value: string) => {
    if (note) setLines((current) => reviseLines(current, value));
    else setText(value);
  };

  const submit = async () => {
    if (busyRef.current) return;
    if (note ? lines.length === 0 : !text.trim()) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    setStale(false);
    const request: V2ComposerSaveRequest = note
      ? {
          kind: "edit",
          noteId: note.id,
          expectedRevision: note.revision,
          lines,
        }
      : {
          kind: capture ? "capture" : "create",
          // A failed create keeps its key so a retry cannot duplicate the Note.
          submissionKey: (submissionKey.current ??= newKey()),
          text,
        };
    try {
      const saved = await v2ComposerSave(request);
      if (note) {
        setLines(
          saved.lines.map((line) => ({ lineId: line.id, text: line.text })),
        );
      } else {
        setText("");
        submissionKey.current = null;
      }
      onSaved?.(saved);
      dock?.onSaved(saved);
    } catch (cause) {
      const message = errorText(cause);
      setError(message);
      setStale(errorKind(cause) === "stale_rev");
      onNotice?.(`Could not save: ${message}`);
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  return (
    <div
      className={`composer${compact ? " compact" : ""}`}
      data-parity="composer"
      data-first-mode={draft.mode ?? undefined}
      data-counts={`${draft.counts.act}-${draft.counts.wait}-${draft.counts.grill}`}
    >
      <ReferenceEditor
        value={draftText}
        placeholder={
          capture
            ? "Write a note, or start with @ a note to add to it."
            : "Write a note. ! acts, ? waits, ?? grills."
        }
        ariaLabel={note ? "Edit note" : "Composer"}
        onChange={changeText}
        onSubmit={() => void submit()}
        onActiveChange={setFocused}
        inputRef={inputRef}
        onPickerChange={reportPicker}
        onPickerTransitionEnd={dock?.onPickerTransitionEnd}
        onEscConsumed={dock?.reportEsc}
      />
      {error && (
        <div className="composer-err" role="alert">
          {error}
        </div>
      )}
      {stale && (
        <button type="button" className="btn ghost" onClick={onReload}>
          Reload saved version
        </button>
      )}
      <div className="composer-actions">
        <span className="meta composer-hint">@ reference</span>
        <span className="meta composer-hint">+ project</span>
        <span className="meta composer-hint">ctrl ↵ send</span>
        <span className="meta composer-count" aria-live="polite">
          {draftLabel(draft)}
        </span>
        {onClose && (
          <button type="button" className="btn ghost" disabled={busy} onClick={onClose}>
            {note ? "Cancel" : "Close composer"}
          </button>
        )}
        <button type="button" className="btn pri composer-submit" disabled={busy} onClick={() => void submit()}>
          {busy ? "Saving…" : note ? "Save changes" : "Capture"}
        </button>
      </div>
    </div>
  );
}
