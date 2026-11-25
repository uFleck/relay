import { useEffect, useRef, useState } from "react";
import { vimStep, type VimCommand } from "../vim";

/* Vim mode for the main window. Normal mode moves a selection through the current view's
   items (elements marked `data-vim-item`) and runs commands; Insert mode is any focused
   text field, and Esc leaves it. Items name their actions with `data-vim-open`,
   `data-vim-edit` and `data-vim-delete`, so each view keeps its own buttons. */

export type VimMode = "normal" | "insert";

type AppCommand = Extract<VimCommand, "newNote" | "goNotebook" | "goTasks" | "goSettings" | "search" | "help">;

interface Options {
  enabled: boolean;
  /** A modal (Search, the cheat sheet) owns the keyboard while it is open. */
  blocked: boolean;
  /** Changes when the view changes, which clears the selection. */
  scope: string;
  onCommand: (command: AppCommand) => void;
}

const PREFIX_MS = 1000;

export const typing = (el: Element | EventTarget | null) =>
  el instanceof HTMLElement && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName));

const pressable = (el: Element | null) =>
  el instanceof HTMLElement && el !== document.body && /^(BUTTON|A|SUMMARY)$/.test(el.tagName);

function items(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>(".body [data-vim-item]")).filter(
    (el) => el.getClientRects().length > 0,
  );
}

function scroller(from: Element | null): HTMLElement | null {
  const start = from ?? document.querySelector(".body [data-vim-item]");
  return start?.closest<HTMLElement>(".scroll") ?? document.querySelector<HTMLElement>(".body .scroll");
}

export function useVim({ enabled, blocked, scope, onCommand }: Options): VimMode {
  const [mode, setMode] = useState<VimMode>(() => (typing(document.activeElement) ? "insert" : "normal"));
  const selected = useRef<HTMLElement | null>(null);
  const pending = useRef("");
  const timer = useRef<number | undefined>(undefined);
  const command = useRef(onCommand);
  command.current = onCommand;

  useEffect(() => {
    const sync = () => setMode(typing(document.activeElement) ? "insert" : "normal");
    // focusout fires before the next element takes focus; read the result a frame later.
    const later = () => requestAnimationFrame(sync);
    document.addEventListener("focusin", sync);
    document.addEventListener("focusout", later);
    return () => {
      document.removeEventListener("focusin", sync);
      document.removeEventListener("focusout", later);
    };
  }, []);

  useEffect(() => {
    selected.current?.removeAttribute("data-vim-on");
    selected.current = null;
  }, [scope, enabled]);

  useEffect(() => {
    if (!enabled) return;

    const select = (el: HTMLElement | undefined) => {
      if (!el) return;
      selected.current?.removeAttribute("data-vim-on");
      el.setAttribute("data-vim-on", "");
      el.scrollIntoView({ block: "nearest" });
      selected.current = el;
    };

    const move = (to: "down" | "up" | "top" | "bottom") => {
      const list = items();
      if (!list.length) return;
      const at = selected.current ? list.indexOf(selected.current) : -1;
      if (to === "top") return select(list[0]);
      if (to === "bottom") return select(list[list.length - 1]);
      if (at < 0) return select(to === "down" ? list[0] : list[list.length - 1]);
      select(list[Math.min(Math.max(at + (to === "down" ? 1 : -1), 0), list.length - 1)]);
    };

    const act = (attr: "data-vim-open" | "data-vim-edit" | "data-vim-delete") => {
      const item = selected.current;
      if (!item?.isConnected) return;
      const target = item.querySelector<HTMLElement>(`[${attr}]`) ?? (attr === "data-vim-open" ? item : null);
      if (target && !(target as HTMLButtonElement).disabled) target.click();
    };

    const onKey = (event: KeyboardEvent) => {
      if (blocked || event.defaultPrevented || document.querySelector(".delete-confirmation")) return;
      if (typing(event.target)) {
        if (event.key === "Escape" && event.target instanceof HTMLElement) {
          event.target.blur();
          setMode("normal");
        }
        return;
      }
      // Holding j or k keeps moving; other keys act once per press.
      if (event.altKey || event.metaKey || (event.repeat && event.key !== "j" && event.key !== "k")) return;
      if (event.key === "Enter" && pressable(document.activeElement)) return;

      const step = vimStep(pending.current, { key: event.key, ctrl: event.ctrlKey });
      window.clearTimeout(timer.current);
      pending.current = step.pending;
      if (step.pending) timer.current = window.setTimeout(() => (pending.current = ""), PREFIX_MS);
      if (!step.handled) return;
      event.preventDefault();

      switch (step.command) {
        case null:
          return;
        case "down":
        case "up":
        case "top":
        case "bottom":
          return move(step.command);
        case "halfDown":
        case "halfUp": {
          const box = scroller(selected.current);
          box?.scrollBy({ top: (step.command === "halfDown" ? 0.5 : -0.5) * box.clientHeight });
          return;
        }
        case "open":
          return act("data-vim-open");
        case "edit":
          return act("data-vim-edit");
        case "delete":
          return act("data-vim-delete");
        default:
          command.current(step.command);
      }
    };

    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.clearTimeout(timer.current);
      pending.current = "";
    };
  }, [enabled, blocked]);

  return mode;
}
