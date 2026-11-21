import { useEffect, useRef, useState } from "react";
import {
  convertUrls,
  copyText,
  findMarks,
  parseUrlValue,
  restoreChip,
  type Chip,
  type EditorState,
  type V2PickerRow,
} from "../references";
import { v2ReferenceParse, v2ReferencePicker } from "../ipc";
import { ReferencePicker } from "./ReferencePicker";

export interface ReferenceEditorProps {
  value: string;
  onChange: (text: string) => void;
  placeholder?: string;
  ariaLabel?: string;
  autoFocus?: boolean;
  multiline?: boolean;
  onSubmit?: () => void;
  onActiveChange?: (active: boolean) => void;
}

/* A textarea with a decoration overlay. The textarea keeps plain chip tokens so caret,
   selection, newlines and native editing stay predictable; the overlay paints them as
   inline chips. Copy, undo and Backspace are handled explicitly below. */
export function ReferenceEditor({
  value,
  onChange,
  placeholder,
  ariaLabel,
  autoFocus,
  multiline = true,
  onSubmit,
  onActiveChange,
}: ReferenceEditorProps) {
  const [state, setState] = useState<EditorState>({ text: value, chips: [] });
  const [rows, setRows] = useState<V2PickerRow[]>([]);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [highlight, setHighlight] = useState(0);
  const [errorToken, setErrorToken] = useState<string | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const undo = useRef<EditorState[]>([]);
  const lastConversion = useRef<{ chip: Chip; delimiter: string; at: number } | null>(null);
  const lastPaste = useRef<{ at: number; previous: EditorState } | null>(null);
  const tokenRange = useRef<{ start: number; end: number } | null>(null);
  const debounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  const generation = useRef(0);

  useEffect(() => {
    if (value === state.text) return;
    setState({ text: value, chips: [] });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);

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

  const closePicker = () => {
    setPickerOpen(false);
    setRows([]);
    setErrorToken(null);
    tokenRange.current = null;
  };

  const runQuery = async (query: string, token: string) => {
    const mine = ++generation.current;
    try {
      const found = await v2ReferencePicker(query);
      if (mine !== generation.current) return;
      setRows(found);
      setHighlight(0);
      setPickerOpen(found.length > 0 || query.length >= 2);
      setErrorToken(found.length === 0 && query.length >= 2 ? `@${token}` : null);
    } catch {
      if (mine === generation.current) closePicker();
    }
  };

  const trackTypedToken = (text: string, caret: number) => {
    let start = caret;
    while (start > 0 && !/\s/.test(text[start - 1])) start -= 1;
    const token = text.slice(start, caret);
    if (debounce.current) clearTimeout(debounce.current);
    if (!token.startsWith("@") || token.length < 2) {
      closePicker();
      return;
    }
    const bare = token.slice(1);
    tokenRange.current = { start, end: caret };
    debounce.current = setTimeout(() => void runQuery(bare, bare), 200);
  };

  const changed = (text: string, caret: number) => {
    lastConversion.current = null;
    lastPaste.current = null;
    const chips = state.chips.filter((chip) => text.includes(chip.token));
    commit({ text, chips });
    trackTypedToken(text, caret);
  };

  const insertChip = (row: V2PickerRow) => {
    const range = tokenRange.current;
    if (!range) return;
    undo.current.push(state);
    lastConversion.current = null;
    lastPaste.current = null;
    const existing = state.chips.find((chip) => chip.key === row.key);
    const chip: Chip = existing ?? {
      key: row.key,
      token: row.displayId,
      kind: row.kind,
      canonicalUrl: row.canonicalLink,
      originalUrl: row.displayId,
    };
    const before = state.text.slice(0, range.start);
    const after = state.text.slice(range.end);
    const next = {
      text: before + chip.token + after,
      chips: existing ? state.chips : [...state.chips, chip],
    };
    commit(next, before.length + chip.token.length);
    closePicker();
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
        closePicker();
        return;
      }
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setHighlight((current) => (rows.length ? (current + 1) % rows.length : 0));
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        setHighlight((current) => (rows.length ? (current - 1 + rows.length) % rows.length : 0));
        return;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        const row = rows[highlight];
        if (row) insertChip(row);
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

  const decorate = () => {
    const nodes: React.ReactNode[] = [];
    let cursor = 0;
    for (const mark of findMarks(state, errorToken)) {
      nodes.push(state.text.slice(cursor, mark.at));
      nodes.push(
        <span
          className={mark.kind === "chip" ? "ref-chip" : "ref-token-error"}
          key={`${mark.key}-${mark.at}`}
        >
          {mark.text}
        </span>,
      );
      cursor = mark.at + mark.text.length;
    }
    nodes.push(state.text.slice(cursor));
    return nodes;
  };

  return (
    <div className="ref-editor">
      <div className="ref-editor-surface">
        <div className="ref-editor-decoration" aria-hidden="true">
          {decorate()}
        </div>
        <textarea
          ref={inputRef}
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
      {pickerOpen && (
        <ReferencePicker
          rows={rows}
          highlight={highlight}
          onHighlight={setHighlight}
          onPick={insertChip}
          onClose={closePicker}
        />
      )}
    </div>
  );
}
