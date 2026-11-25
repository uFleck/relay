/* Vim mode's key grammar, kept free of the DOM so it can be tested. Normal mode turns key
   presses into commands; a prefix key (g, d) waits for the next key. */

export type VimCommand =
  | "down"
  | "up"
  | "top"
  | "bottom"
  | "halfDown"
  | "halfUp"
  | "open"
  | "edit"
  | "delete"
  | "newNote"
  | "goNotebook"
  | "goTasks"
  | "goSettings"
  | "search"
  | "help";

export interface VimKey {
  key: string;
  ctrl: boolean;
}

const SINGLE: Record<string, VimCommand> = {
  j: "down",
  k: "up",
  G: "bottom",
  Enter: "open",
  o: "open",
  e: "edit",
  i: "newNote",
  a: "newNote",
  "/": "search",
  "?": "help",
};

const PAIRS: Record<string, VimCommand> = {
  gg: "top",
  gn: "goNotebook",
  gt: "goTasks",
  gs: "goSettings",
  dd: "delete",
};

const PREFIXES = new Set(["g", "d"]);

export interface VimStep {
  /** The command this key completes, if any. */
  command: VimCommand | null;
  /** The prefix still waiting for its second key. */
  pending: string;
  /** True when the key belongs to Vim mode and its default action should be suppressed. */
  handled: boolean;
}

export function vimStep(pending: string, { key, ctrl }: VimKey): VimStep {
  if (ctrl) {
    const command = key === "d" ? "halfDown" : key === "u" ? "halfUp" : null;
    return { command, pending: "", handled: command !== null };
  }
  if (pending) {
    const command = PAIRS[pending + key] ?? null;
    return { command, pending: "", handled: true };
  }
  if (PREFIXES.has(key)) return { command: null, pending: key, handled: true };
  const command = SINGLE[key] ?? null;
  return { command, pending: "", handled: command !== null };
}

/** The cheat sheet `?` opens, in the order it is shown. */
export const VIM_HELP: { keys: string; does: string }[] = [
  { keys: "j / k", does: "Next / previous item" },
  { keys: "gg / G", does: "First / last item" },
  { keys: "ctrl+d / ctrl+u", does: "Scroll half a page" },
  { keys: "enter / o", does: "Open the item" },
  { keys: "e", does: "Edit the note" },
  { keys: "dd", does: "Delete the item" },
  { keys: "i / a", does: "New note" },
  { keys: "gn / gt / gs", does: "Notebook / Tasks / Settings" },
  { keys: "/", does: "Search" },
  { keys: "esc", does: "Leave a field (Insert → Normal)" },
  { keys: "?", does: "This list" },
];
