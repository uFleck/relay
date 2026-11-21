import { Icon, Mark, type IconName } from "../components/Icon";
import type { View } from "../route";

/* The v2 rail: Notebook, Tasks and Settings. The single Needs-you count is owned by #27;
   this transition rail shows the count when one is supplied. */

interface Props {
  view: View;
  onView: (v: View) => void;
  version: string;
  needsYou?: number;
  /** The update notice, above Settings. */
  notice?: React.ReactNode;
}

export function Rail({ view, onView, version, needsYou, notice }: Props) {
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
    <nav className="rail" aria-label="Views">
      <div className="mark">
        <Mark />
        Relay<span className="ver meta">{version}</span>
      </div>
      {item("notebook", "pencil", "Notebook")}
      {item("tasks", "check", "Tasks", !!needsYou && <span className="bdg warn">{needsYou}</span>)}
      <div className="sp" />
      {notice}
      {item("settings", "gear", "Settings")}
    </nav>
  );
}
