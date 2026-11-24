import {
  cacheAge,
  groupRows,
  GROUP_LABEL,
  type ProviderProgress,
  type V2PickerRow,
} from "../references";

export interface ReferencePickerProps {
  rows: V2PickerRow[];
  highlight: number;
  onHighlight: (index: number) => void;
  onPick: (row: V2PickerRow) => void;
  onClose: () => void;
  /** Provider search feedback. Local rows are always rendered beneath it. */
  progress?: ProviderProgress[];
  /** After roughly three seconds the spinner becomes a quiet status line. */
  slow?: boolean;
  /** Kept mounted so open and close both animate; CSS collapses the closed box. */
  open: boolean;
  onTransitionEnd?: () => void;
}

/* Keyboard focus stays in the editor; the highlighted row drives the Lens on the right. */
export function ReferencePicker({
  rows,
  highlight,
  onHighlight,
  onPick,
  onClose,
  progress = [],
  slow = false,
  open,
  onTransitionEnd,
}: ReferencePickerProps) {
  const flat = rows;
  const lens = flat[Math.min(highlight, Math.max(0, flat.length - 1))];
  let index = -1;

  return (
    <div
      className="ref-picker"
      data-open={open ? "true" : "false"}
      data-lens={open && flat.length > 0 ? "true" : "false"}
      role="listbox"
      aria-label="References"
      aria-hidden={!open}
      inert={open ? undefined : true}
      onTransitionEnd={(event) => {
        if (event.target === event.currentTarget && event.propertyName === "max-height") {
          onTransitionEnd?.();
        }
      }}
    >
      <div className="ref-picker-results">
        {progress
          .filter((item) => item.state !== "ok")
          .map((item) => (
            <div className={`ref-provider ref-provider-${item.state}`} key={item.provider}>
              <span className="ref-provider-name">{item.provider}</span>
              <span className="ref-provider-text">
                {item.state === "unavailable"
                  ? item.reason ?? "Provider unavailable"
                  : slow
                    ? "Still searching…"
                    : "Searching…"}
              </span>
              {item.state === "unavailable" && item.settingsDestination && (
                <span className="ref-provider-destination">{item.settingsDestination}</span>
              )}
            </div>
          ))}
        {groupRows(rows).map((section) => (
          <div className="ref-picker-group" key={section.group}>
            <div className="ref-picker-group-title">{GROUP_LABEL[section.group]}</div>
            {section.rows.map((row) => {
              index += 1;
              const mine = index;
              return (
                <button
                  type="button"
                  role="option"
                  aria-selected={mine === highlight}
                  className={`ref-picker-row${mine === highlight ? " active" : ""}${
                    row.availability === "unavailable" ? " unavailable" : ""
                  }`}
                  key={row.key}
                  onMouseEnter={() => onHighlight(mine)}
                  onClick={() => onPick(row)}
                >
                  <span className="ref-picker-id">{row.displayId}</span>
                  <span className="ref-picker-title">{row.title}</span>
                  <span className="ref-picker-kind">{row.kind}</span>
                </button>
              );
            })}
          </div>
        ))}
      </div>
      <aside className="ref-lens" aria-label="Reference preview">
        {lens ? (
          <>
            <div className="ref-lens-head">
              <span className="ref-lens-kind">{lens.kind}</span>
              <span className="ref-lens-id">{lens.displayId}</span>
              <button type="button" className="ref-lens-close" aria-label="Close picker" onClick={onClose}>
                ×
              </button>
            </div>
            <div className="ref-lens-title">{lens.title}</div>
            {lens.availability === "unavailable" ? (
              <div className="ref-lens-unavailable">
                <div>{lens.unavailableReason ?? "Target unavailable"}</div>
                {lens.snapshot?.content && <div className="ref-lens-content">{lens.snapshot.content}</div>}
                <div className="ref-lens-meta">{cacheAge(lens.snapshot?.cachedAt ?? null)}</div>
                {lens.settingsDestination && (
                  <div className="ref-lens-destination">{lens.settingsDestination}</div>
                )}
              </div>
            ) : (
              <>
                {lens.state && <div className="ref-lens-state">{lens.state}</div>}
                <div className="ref-lens-metadata">
                  {lens.metadata.slice(0, 3).map((line, at) => (
                    <div key={at}>{line}</div>
                  ))}
                  {lens.snapshot?.content && <div>{lens.snapshot.content}</div>}
                </div>
                <div className="ref-lens-meta">{cacheAge(lens.snapshot?.cachedAt ?? null)}</div>
                <div className="ref-lens-readonly">read-only · the agent does any writes</div>
              </>
            )}
            <div className="ref-lens-link" title={lens.canonicalLink}>
              {lens.canonicalLink}
            </div>
          </>
        ) : (
          <div className="ref-lens-empty">No local match</div>
        )}
      </aside>
    </div>
  );
}
