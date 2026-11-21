import { createContext, useContext, type RefObject } from "react";
import type { DraftSummary } from "../directives";
import type { V2Note } from "../ipc";

/* Contract handed to #39 (native Dock sizing and Windows focus). The webview owns these
   DOM nodes and states; Rust only sizes and positions the transparent window box. */

export type DockLayoutState = "compact" | "picker" | "picker-lens";

/** The chosen prototype's preferred width, clamped to the native window's available width. */
export const DOCK_PREFERRED_WIDTH = 760;

/** The picker floats above the input; its settled box and gap must be added to the measured root. */
export const PICKER_MAX_HEIGHT = 320;
export const PICKER_GAP = 6;

/** The CSS height the native window needs, rounded up once before it becomes physical pixels. */
export function dockCssHeight(
  contentHeight: number,
  pickerHeight: number,
  layout: DockLayoutState,
): number {
  const extra = layout === "compact" ? 0 : Math.min(pickerHeight, PICKER_MAX_HEIGHT) + PICKER_GAP;
  return Math.ceil(contentHeight + extra);
}

/** Registered in src-tauri/src/lib.rs as `ctrl+alt+r`. */
export const SUMMON_CHORD = "Ctrl+Alt+R";

export function dockLayoutState(pickerOpen: boolean, lensVisible: boolean): DockLayoutState {
  if (!pickerOpen) return "compact";
  return lensVisible ? "picker-lens" : "picker";
}

export interface DockBridge {
  /** Bottom-aligned, clipped viewport. Its box is what Rust sizes. */
  viewportRef: RefObject<HTMLDivElement | null>;
  /** Untransformed content root. Measure this, never the animated viewport children. */
  contentRef: RefObject<HTMLDivElement | null>;
  /** The live Composer textarea, forwarded through Composer → ReferenceEditor. */
  inputRef: RefObject<HTMLTextAreaElement | null>;
  layout: DockLayoutState;
  /** Called by the create-mode Composer when its picker opens or closes. */
  reportPicker: (open: boolean, lensVisible: boolean) => void;
  /** True from the moment the picker consumes Escape until `takeEsc` reads it. */
  escConsumed: RefObject<boolean>;
  reportEsc: () => void;
  takeEsc: () => boolean;
  /** Fires when the picker's open/close transition finishes, after layout.state settles. */
  onPickerTransitionEnd: () => void;
  /** Fires on a successful create or edit save. */
  onSaved: (note: V2Note) => void;
  /** The current draft's first mode and per-mode counts. */
  reportDraft: (draft: DraftSummary) => void;
}

export const DockContext = createContext<DockBridge | null>(null);

export const useDock = () => useContext(DockContext);
