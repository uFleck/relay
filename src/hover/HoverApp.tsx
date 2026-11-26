import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { EMPTY_DRAFT, type DraftSummary } from "../directives";
import { Composer } from "../components/Composer";
import { Icon } from "../components/Icon";
import {
  dockDismiss,
  dockReady,
  dockResize,
  inTauri,
  onDockDpi,
  onDockFocus,
  onEvent,
  showMainWindow,
  type HoverFlags,
} from "../ipc";
import type { HoverEdge } from "../types";
import {
  DockContext,
  dockCssHeight,
  dockLayoutState,
  type DockBridge,
  type DockLayoutState,
} from "./dock";
import { useKeybinds } from "../keybinds";
import { useWorkspace } from "../workspace";

/* The Dock only captures: a new note, or lines appended to an existing one. Tasks, updates
   and the rest of Relay live in the main window. The native session and focus machinery
   below sizes the window to the content, so the Dock never scrolls. */

const REFRESH_MS = 20_000;

export function HoverApp() {
  const { workspace: v2, error: workspaceError, refresh: refreshV2 } = useWorkspace();
  const contentReady = v2 !== null;
  const failed = workspaceError !== null;
  const [flags, setFlags] = useState<HoverFlags>({ pinned: false, paused: false, ready: false, visible: false });
  const [dockLayout, setDockLayout] = useState<DockLayoutState>("compact");
  const [draft, setDraft] = useState<DraftSummary>(EMPTY_DRAFT);
  const [dockVisible, setDockVisible] = useState(!inTauri);
  const [focusHint, setFocusHint] = useState(false);
  const [growing, setGrowing] = useState(false);
  const shell = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const escConsumed = useRef(false);
  // Native session plumbing. Every queued measurement carries the session id it started with.
  const sessionRef = useRef<number | null>(null);
  const layoutRef = useRef(dockLayout);
  layoutRef.current = dockLayout;
  const pickerOpenRef = useRef(false);
  const sentHeight = useRef<number | null>(null);
  const readySent = useRef(false);
  /** Explicit dismissal keeps the draft: the Composer stays mounted and only the native window hides. */
  const dismissDock = useCallback(async (reason: "escape" | "saved") => {
    const id = sessionRef.current;
    if (id === null) return;
    sessionRef.current = null;
    setDockVisible(false);
    setFocusHint(false);
    await dockDismiss(id, reason).catch(() => undefined);
  }, []);

  /** The picker sits above the input, outside the measured content box. */
  const measureHeight = useCallback(() => {
    const content = contentRef.current;
    if (!content) return 0;
    const picker = content.querySelector<HTMLElement>(".ref-picker");
    const pickerHeight = picker?.offsetHeight ?? 0;
    const measured = Math.max(content.offsetHeight, content.scrollHeight);
    return dockCssHeight(measured, pickerHeight, layoutRef.current);
  }, []);

  const applyHeight = useCallback(
    async (force = false) => {
      if (!inTauri) return;
      const height = measureHeight();
      if (!height) return;
      if (!readySent.current) {
        readySent.current = true;
        sentHeight.current = height;
        await dockReady(height).catch(() => undefined);
        return;
      }
      const previous = sentHeight.current;
      if (!force && previous === height) return;
      const id = sessionRef.current;
      if (id === null) {
        // No session to resize. Re-measure on the next summon instead.
        sentHeight.current = null;
        return;
      }
      const growing = previous === null || height > previous;
      // Record the request before awaiting it, so a quick close can supersede an open.
      sentHeight.current = height;
      if (growing) setGrowing(true);
      const ok = await dockResize(height, id).catch(() => false);
      if (sessionRef.current !== id || sentHeight.current !== height) return;
      if (!ok) sentHeight.current = null;
      setGrowing(false);
    },
    [measureHeight],
  );

  /* Everything #39 needs to size and focus the native box: the measured content root, the
     three layout states, the live input and the save/escape signals. */
  const dock = useMemo<DockBridge>(
    () => ({
      viewportRef,
      contentRef,
      inputRef,
      layout: dockLayout,
      reportPicker: (open, lensVisible) => {
        pickerOpenRef.current = open;
        const next = dockLayoutState(open, lensVisible);
        setDockLayout((current) => (current === next ? current : next));
      },
      escConsumed,
      reportEsc: () => {
        escConsumed.current = true;
      },
      takeEsc: () => {
        const taken = escConsumed.current;
        escConsumed.current = false;
        return taken;
      },
      onSaved: () => {
        void dismissDock("saved");
        void refreshV2();
      },
      reportDraft: setDraft,
    }),
    [dockLayout, refreshV2, dismissDock],
  );

  useEffect(() => {
    const timer = window.setInterval(() => void refreshV2(), REFRESH_MS);
    const offState = onEvent("relay://state", setFlags);
    return () => {
      window.clearInterval(timer);
      offState();
    };
  }, [refreshV2]);

  // Native activation is verified in Rust before this event arrives. A refused activation leaves
  // the Dock clickable with a hint; an ended session hides it without clearing the draft.
  useEffect(() => {
    if (!inTauri) return;
    return onDockFocus((event) => {
      sessionRef.current = event.visible ? event.sessionId : null;
      setDockVisible(event.visible);
      setFocusHint(event.hint === "click-to-focus");
      setGrowing(false);
      if (!event.visible) {
        sentHeight.current = null;
        inputRef.current?.blur();
        return;
      }
      // Re-measure for this session: the draft may have grown while the Dock was hidden.
      void applyHeight(true);
    });
  }, [applyHeight]);

  useLayoutEffect(() => {
    if (!dockVisible || !contentReady) return;
    const focusInput = () => {
      if (document.activeElement === document.body) inputRef.current?.focus({ preventScroll: true });
    };
    focusInput();
    window.addEventListener("focus", focusInput);
    return () => window.removeEventListener("focus", focusInput);
  }, [dockVisible, contentReady]);

  useEffect(() => {
    if (!inTauri) return;
    return onDockDpi((event) => {
      if (sessionRef.current !== event.sessionId) return;
      // Rust has already re-anchored to the new work area; measure once the webview has reflowed.
      requestAnimationFrame(() => void applyHeight(true));
    });
  }, [applyHeight]);

  // The measurable content root only exists once the v2 read resolves, so the observer must attach
  // when that happens and not only on the first mount (which renders the loading tab instead).
  useEffect(() => {
    const content = contentRef.current;
    if (!content || !inTauri) return;
    let frame = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => void applyHeight());
    });
    observer.observe(content);
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
    };
  }, [applyHeight, contentReady]);

  useEffect(() => {
    void applyHeight();
  }, [dockLayout, applyHeight, contentReady]);

  useEffect(() => {
    if (!inTauri) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || pickerOpenRef.current) return;
      event.preventDefault();
      void dismissDock("escape");
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [dismissDock]);

  const shownEdge: HoverEdge = "right";

  if (!v2) {
    // Nothing to count yet: a quiet tab that still places itself, so the window can be shown.
    return (
      <div
        className={`hroot${inTauri ? "" : " dev"}`}
        data-edge={shownEdge}
        data-mode="tab"
        data-visible={dockVisible ? "true" : "false"}
        data-growing="false"
      >
        <div ref={shell} className="tab" aria-label={failed ? "Relay: data unavailable" : "Relay: loading"}>
          <span className="cnt">{failed ? "!" : "·"}</span>
        </div>
      </div>
    );
  }

  return (
    <div
      className={`hroot${inTauri ? "" : " dev"}`}
      data-edge={shownEdge}
      data-mode="open"
      data-visible={dockVisible ? "true" : "false"}
      data-growing={growing ? "true" : "false"}
      data-focus-hint={focusHint ? "click-to-focus" : undefined}
    >
      <div
        ref={shell}
        className="hov dock"
        data-compose="on"
        data-dock-state={dockLayout}
      >
        <Panel dock={dock} draftMode={draft.mode} paused={flags.paused} failed={failed} focusHint={focusHint} />
      </div>
    </div>
  );
}

interface PanelProps {
  dock: DockBridge;
  draftMode: DraftSummary["mode"];
  paused: boolean;
  failed: boolean;
  focusHint: boolean;
}

function Panel({ dock, draftMode, paused, failed, focusHint }: PanelProps) {
  const { summon } = useKeybinds();
  return (
    <DockContext.Provider value={dock}>
      <div className="dock-viewport" ref={dock.viewportRef}>
        <div className="dock-scroll">
          <div className="dock-content" ref={dock.contentRef} data-layout={dock.layout} data-mode={draftMode ?? undefined}>
            <section className="hcomposer" aria-label="Capture">
              <Composer compact capture autoFocus />
            </section>
            <div className="hfoot">
              {failed && <div className="slot warn-fg">Could not read Relay's data · retrying</div>}
              <div className="hf">
                <span>
                  <kbd>ctrl</kbd>+<kbd>↵</kbd> send
                </span>
                <span>
                  <kbd>esc</kbd> dismiss
                </span>
                <span>
                  <kbd title="Summon">{summon}</kbd>
                </span>
                {paused && <span className="warn-fg">capture paused</span>}
                {focusHint && <span className="focus-hint" role="status">Click to focus</span>}
                <button type="button" className="linkish open" onClick={() => void showMainWindow()}>
                  Open Relay
                  <Icon name="ext" small />
                </button>
              </div>
            </div>
          </div>
        </div>
      </div>
    </DockContext.Provider>
  );
}
