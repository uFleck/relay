import { Icon, Mark, type IconName } from "../components/Icon";
import type { View } from "../route";
import type { SyncSource } from "../types";

interface Props {
  view: View;
  onView: (v: View) => void;
  version: string;
  inbox: number;
  review: number;
  liveAgents: number;
  sync: SyncSource[];
  /** The update notice, above the sync dots. */
  notice?: React.ReactNode;
}

export function Rail({ view, onView, version, inbox, review, liveAgents, sync, notice }: Props) {
  const item = (v: View, icon: IconName, label: string, trail?: React.ReactNode) => (
    <button type="button" className={`nav${view === v ? " on" : ""}`} aria-current={view === v ? "page" : undefined} onClick={() => onView(v)}>
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
      {item("today", "sun", "Today")}
      {item("inbox", "tray", "Inbox", inbox > 0 && <span className="bdg">{inbox}</span>)}
      {item("projects", "folders", "Projects")}
      {item("agents", "robot", "Agents", liveAgents > 0 && <span className="meta push">{liveAgents} live</span>)}
      {item("review", "review", "Review", review > 0 && <span className="bdg warn">{review}</span>)}
      <div className="sp" />
      {notice}
      <div className="hint">
        <SyncDots sources={sync} />
      </div>
      {item("settings", "gear", "Settings")}
    </nav>
  );
}

export function SyncDots({ sources }: { sources: SyncSource[] }) {
  return (
    <div className="sync meta">
      {sources.length === 0 && <span title="Slack, JEV and the trackers are not connected yet">no sources yet</span>}
      {sources.map((s) => (
        <span key={s.name} title={s.reason ? `${s.name}: ${s.reason}` : `${s.name}: ok`}>
          <i className={`dot sync-${s.state}`} />
          {s.short}
        </span>
      ))}
    </div>
  );
}
