import { useEffect, useRef, useState } from "react";
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

interface DraftLine {
  key: string;
  lineId: string | null;
  text: string;
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
  autoFocus?: boolean;
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
  autoFocus,
}: ComposerProps) {
  const [text, setText] = useState("");
  const [lines, setLines] = useState<DraftLine[]>(() =>
    (note?.lines ?? []).map((line) => ({ key: line.id, lineId: line.id, text: line.text })),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [stale, setStale] = useState(false);
  const [focused, setFocused] = useState(false);
  const busyRef = useRef(false);
  const submissionKey = useRef<string | null>(null);

  useEffect(() => {
    onActiveChange?.(focused || busy);
  }, [focused, busy, onActiveChange]);

  const changeLine = (key: string, value: string) =>
    setLines((current) => current.map((line) => (line.key === key ? { ...line, text: value } : line)));
  const addLine = () => setLines((current) => [...current, { key: newKey(), lineId: null, text: "" }]);
  const removeLine = (key: string) => setLines((current) => current.filter((line) => line.key !== key));

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
          lines: lines.map(
            ({ lineId, text }): V2LineInput => ({ lineId, text }),
          ),
        }
      : {
          kind: "create",
          // A failed create keeps its key so a retry cannot duplicate the Note.
          submissionKey: (submissionKey.current ??= newKey()),
          text,
        };
    try {
      const saved = await v2ComposerSave(request);
      if (note) {
        setLines(
          saved.lines.map((line) => ({ key: line.id, lineId: line.id, text: line.text })),
        );
      } else {
        setText("");
        submissionKey.current = null;
      }
      onSaved?.(saved);
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
    <div className={`composer${compact ? " compact" : ""}`}>
      {note ? (
        <div className="composer-lines">
          {lines.map((line) => (
            <div className="composer-line" key={line.key}>
              <div data-line-id={line.lineId ?? "new"} className="composer-line-editor">
                <ReferenceEditor
                  multiline={false}
                  value={line.text}
                  ariaLabel="Note line"
                  placeholder="! Act · ? Wait · ?? Grill · plain text"
                  onChange={(text) => changeLine(line.key, text)}
                  onSubmit={() => void submit()}
                  onActiveChange={setFocused}
                />
              </div>
              <button
                type="button"
                className="ib"
                title="Remove line"
                aria-label="Remove line"
                onClick={() => removeLine(line.key)}
              >
                ×
              </button>
            </div>
          ))}
          <button type="button" className="btn ghost" onClick={addLine}>
            + Add line
          </button>
        </div>
      ) : (
        <ReferenceEditor
          value={text}
          placeholder="Capture a note. ! Act · ? Wait · ?? Grill"
          ariaLabel="Composer"
          autoFocus={autoFocus}
          onChange={setText}
          onSubmit={() => void submit()}
          onActiveChange={setFocused}
        />
      )}
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
        <span className="meta">@ reference · + project · ctrl ↵ send</span>
        <button type="button" className="btn pri" disabled={busy} onClick={() => void submit()}>
          {busy ? "Saving…" : note ? "Save changes" : "Capture"}
        </button>
      </div>
    </div>
  );
}
