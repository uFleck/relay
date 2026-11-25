import { Fragment } from "react";
import type { V2AgentMode } from "../ipc";
import { Icon, type IconName } from "../components/Icon";
import { useKeybinds } from "../keybinds";
import type { View } from "../route";

/* The v2 rail: Notebook, Tasks and Settings. The single Needs-you count is owned by #27;
   this rail shows the count when one is supplied. The wordmark dot follows the draft's
   first directive and pops when that directive changes. */

interface Props {
  view: View;
  onView: (v: View) => void;
  version: string;
  needsYou?: number;
  /** The update notice, above Settings. */
  notice?: React.ReactNode;
  wordmarkMode?: V2AgentMode | null;
}

export function Rail({ view, onView, version, needsYou, notice, wordmarkMode }: Props) {
  const { summon } = useKeybinds();
  const item = (v: View, icon: IconName, label: string, trail?: React.ReactNode) => (
    <button
      type="button"
      className={`nav${view === v ? " on" : ""}`}
      aria-current={view === v ? "page" : undefined}
      onClick={() => onView(v)}
    >
      <Icon name={icon} />
      {label}
      {trail}
    </button>
  );

  return (
    <nav className="rail" aria-label="Views" data-parity="rail">
      <div className="mark" data-mode={wordmarkMode ?? undefined} data-tauri-drag-region>
        <span className="wordmark" data-tauri-drag-region>
          relay<span className="dot" key={wordmarkMode ?? "none"}>.</span>
        </span>
        <span className="ver meta">{version}</span>
      </div>
      {item("notebook", "pencil", "Notebook")}
      {item(
        "tasks",
        "check",
        "Tasks",
        !!needsYou && (
          <span className="bdg warn" aria-label={`${needsYou} tasks need you`}>
            {needsYou}
          </span>
        ),
      )}
      {item("settings", "gear", "Settings")}
      <div className="sp" />
      {notice}
      <div className="hint meta">
        <div>
          {summon.split("+").map((key, index) => (
            <Fragment key={`${key}-${index}`}>
              {index > 0 && "+"}
              <kbd>{key.toLowerCase()}</kbd>
            </Fragment>
          ))}{" "}
          summon
        </div>
        <div>
          <kbd>ctrl</kbd>+<kbd>↵</kbd> send
        </div>
        <div>
          <kbd>esc</kbd> dismiss
        </div>
      </div>
    </nav>
  );
}
