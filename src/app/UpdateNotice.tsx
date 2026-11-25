import { useEffect, useId, useState } from "react";
import { Icon } from "../components/Icon";
import { checkForUpdates, downloadUpdate, installUpdate, type ReleaseNote, type UpdateState } from "../updater";
import { changelogLines } from "./changelog";

/* The Rail's update notice: one line that names the next step and runs it on click. Hover
   or focus shows what changed. The version label under it runs a manual check. */

function noticeLabel(update: UpdateState): string | null {
  switch (update.kind) {
    case "available":
      return `${update.version} available`;
    case "downloading":
      return update.progress === null ? "Downloading…" : `Downloading ${Math.round(update.progress * 100)}%`;
    case "ready":
      return `Restart for ${update.version}`;
    case "error":
      return "Update failed · retry";
    default:
      return null;
  }
}

function runNext(update: UpdateState) {
  if (update.kind === "available") void downloadUpdate();
  else if (update.kind === "ready") void installUpdate();
  else if (update.kind === "error") void checkForUpdates(true);
}

const shortDate = (iso: string) => {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleDateString("en-GB", { day: "numeric", month: "short" });
};

function Changelog({ id, notes }: { id: string; notes: ReleaseNote[] }) {
  return (
    <div className="un-tip" id={id} role="tooltip">
      {notes.map((note) => (
        <section key={note.version} className="un-rel">
          <header>
            <b>{note.version}</b>
            <span className="meta">{shortDate(note.date)}</span>
          </header>
          {changelogLines(note.body).map((line, index) =>
            line.kind === "head" ? (
              <h4 key={index}>{line.text}</h4>
            ) : (
              <p key={index} className={line.kind === "item" ? "it" : undefined}>
                {line.text}
              </p>
            ),
          )}
        </section>
      ))}
    </div>
  );
}

export function UpdateNotice({ update }: { update: UpdateState }) {
  const tipId = useId();
  const label = noticeLabel(update);
  if (!label) return null;
  const notes = "notes" in update ? update.notes : [];
  const busy = update.kind === "downloading";
  return (
    <div className="unote" data-kind={update.kind}>
      <button
        type="button"
        className="un-btn"
        disabled={busy}
        aria-describedby={notes.length ? tipId : undefined}
        title={update.kind === "error" ? update.message : undefined}
        onClick={() => runNext(update)}
      >
        <Icon name={update.kind === "ready" ? "refresh" : "download"} small />
        <span>{label}</span>
      </button>
      {notes.length > 0 && <Changelog id={tipId} notes={notes} />}
    </div>
  );
}

/** The running version. A click checks for updates and says what it found. */
export function VersionCheck({ version, update }: { version: string; update: UpdateState }) {
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    if (!result) return;
    const timer = window.setTimeout(() => setResult(null), 4000);
    return () => window.clearTimeout(timer);
  }, [result]);

  const check = async () => {
    const outcome = await checkForUpdates(true);
    setResult(outcome === "none" ? "up to date" : outcome === "failed" ? "check failed" : null);
  };

  return (
    <button type="button" className="rail-ver meta" title="Check for updates" onClick={() => void check()}>
      {version ? `v${version}` : "version"}
      {update.kind === "checking" ? " · checking…" : result ? ` · ${result}` : ""}
    </button>
  );
}
