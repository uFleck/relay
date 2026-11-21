import { Icon } from "../components/Icon";
import { checkForUpdates, downloadUpdate, installUpdate, type UpdateState } from "../updater";

/** The action that fits the updater's state; the updater itself refuses overlapping work. */
export function UpdateActions({ update }: { update: UpdateState }) {
  if (update.kind === "available") {
    return (
      <button type="button" className="btn pri" onClick={() => void downloadUpdate()}>
        <Icon name="download" small />
        Download
      </button>
    );
  }
  if (update.kind === "ready") {
    return (
      <button type="button" className="btn pri" onClick={() => void installUpdate()}>
        Restart to update
      </button>
    );
  }
  const busy = update.kind === "checking" || update.kind === "downloading";
  return (
    <button type="button" className="btn" disabled={busy} onClick={() => void checkForUpdates(true)}>
      {update.kind === "checking" ? "Checking…" : update.kind === "error" ? "Try again" : "Check for updates"}
    </button>
  );
}
