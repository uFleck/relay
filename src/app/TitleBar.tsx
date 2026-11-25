import { useState } from "react";
import { Icon } from "../components/Icon";
import { inTauri } from "../ipc";

/* The title bar spans the main column. Pages fill its title and actions slots through
   PageBar; the window controls stay on the right. `data-tauri-drag-region` must sit on each
   element that drags, not only the parent. */

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
  onTitleSlot?: (element: HTMLDivElement | null) => void;
  onActionsSlot?: (element: HTMLDivElement | null) => void;
  onSearch?: () => void;
}

export function TitleBar({ onTitleSlot, onActionsSlot, onSearch }: Props) {
  const [maximized, setMaximized] = useState(false);

  return (
    <header className="title" data-tauri-drag-region>
      <div className="tb-page" ref={onTitleSlot} data-tauri-drag-region />
      <div className="tb-actions" ref={onActionsSlot} />
      {onSearch && (
        <button type="button" className="ib tb-search" title="Search (Ctrl K)" aria-label="Search" onClick={onSearch}>
          <Icon name="search" small />
        </button>
      )}
      <span className="tb-div" aria-hidden="true" />
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
}
