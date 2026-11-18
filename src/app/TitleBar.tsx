import { forwardRef, useState } from "react";
import { Icon } from "../components/Icon";
import { inTauri } from "../ipc";

/* 36px custom title bar. `data-tauri-drag-region` must sit on each element that drags, not only the parent. */

async function withWindow(action: "minimize" | "toggleMaximize" | "close") {
  if (!inTauri) return false;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    await win[action]();
    return action === "toggleMaximize" ? await win.isMaximized() : false;
  } catch (error) {
    console.warn(`[relay] window.${action} failed`, error);
    return false;
  }
}

interface Props {
  query: string;
  onQuery: (q: string) => void;
}

export const TitleBar = forwardRef<HTMLInputElement, Props>(function TitleBar({ query, onQuery }, searchRef) {
  const [maximized, setMaximized] = useState(false);

  return (
    <header className="title" data-tauri-drag-region>
      <Icon name="relay" small className="accent-fg" />
      <span className="name" data-tauri-drag-region>
        Relay
      </span>
      <label className="search">
        <Icon name="search" small />
        <input
          ref={searchRef}
          value={query}
          onChange={(e) => onQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.stopPropagation();
              onQuery("");
              e.currentTarget.blur();
            }
          }}
          placeholder="Search tasks, changes, threads"
          spellCheck={false}
          aria-label="Search"
        />
        <kbd>Ctrl K</kbd>
      </label>
      <div className="wc">
        <button type="button" title="Minimize" onClick={() => void withWindow("minimize")}>
          <Icon name="min" small />
        </button>
        <button
          type="button"
          title={maximized ? "Restore" : "Maximize"}
          onClick={() => void withWindow("toggleMaximize").then(setMaximized)}
        >
          <Icon name={maximized ? "restore" : "max"} small />
        </button>
        <button type="button" className="x" title="Close" onClick={() => void withWindow("close")}>
          <Icon name="x" small />
        </button>
      </div>
    </header>
  );
});
