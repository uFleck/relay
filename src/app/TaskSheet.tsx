import { useEffect, useRef } from "react";
import { Icon } from "../components/Icon";
import { SIGIL } from "../workspace";
import type { V2Note, V2Project, V2Task } from "../ipc";

/* The read-only v2 Task sheet. App resolves the Task by its routed id from the unfiltered
   workspace on every committed change, so an open sheet refreshes without the filter moving
   it. Live Brief editing, approvals, questions, retry, Follow-up and closure belong to the
   #34-#37 producers; this surface only reads what they persisted. */

interface Props {
  task: V2Task;
  project: V2Project | undefined;
  note: V2Note | undefined;
  onClose: () => void;
}

export function TaskSheet({ task, project, note, onClose }: Props) {
  const sheetRef = useRef<HTMLElement>(null);
  const sourceLine = note?.lines.find((line) => line.id === task.sourceLineId)?.text;

  // Keyboard users land in the sheet; App's Esc closes it and restores focus to the row.
  useEffect(() => {
    sheetRef.current?.focus({ preventScroll: true });
  }, [task.id]);

  const failure = task.failureStage || task.failureReason;

  return (
    <aside
      ref={sheetRef}
      className="sheet v2-sheet"
      tabIndex={-1}
      aria-label={`Task ${task.originalLine}`}
      data-task-id={task.id}
    >
      <div className="sh-h">
        <div className="sh-top">
          <span className={`tag mode-${task.agentMode}`}>
            {SIGIL[task.agentMode]} {task.agentMode}
          </span>
          <span className="meta mono">{task.id}</span>
          <span className="push" />
          <button type="button" className="ib" title="Close (Esc)" onClick={onClose}>
            <Icon name="x" small />
          </button>
        </div>
        <h2>{task.originalLine}</h2>
        <div className="meta">
          {task.status.replace(/_/g, " ")}
          {task.status === "grilling" && task.grillTurn && task.grillTurn !== "agent"
            ? ` · ${task.grillTurn.replace(/_/g, " ")}`
            : ""}
        </div>
      </div>

      <div className="sh-b">
        {sourceLine && sourceLine !== task.originalLine && (
          <section className="sec">
            <h3>Source line</h3>
            <p className="desc">{sourceLine}</p>
          </section>
        )}

        {project && (
          <section className="sec">
            <h3>Project</h3>
            <p className="desc">{project.name}</p>
          </section>
        )}

        <section className="sec">
          <h3>References</h3>
          {task.references.length === 0 ? (
            <p className="desc">None.</p>
          ) : (
            <div className="nb-refs">
              {task.references.map((reference) => (
                <span className="nb-ref" key={reference.id} title={reference.canonicalUrl}>
                  @{reference.targetId}
                </span>
              ))}
            </div>
          )}
        </section>

        <section className="sec">
          <h3>Brief</h3>
          <p className="desc">{task.brief ?? "No Brief yet."}</p>
        </section>

        {task.pendingAction && (
          <section className="sec">
            <h3>Pending action</h3>
            <p className="desc">{task.pendingAction}</p>
          </section>
        )}

        {failure && (
          <section className="sec">
            <h3>Failure</h3>
            {task.failureStage && <p className="desc">{task.failureStage}</p>}
            {task.failureReason && <p className="desc">{task.failureReason}</p>}
          </section>
        )}

        {task.threadUrl && (
          <section className="sec">
            <h3>Thread</h3>
            <a className="lnk" href={task.threadUrl} target="_blank" rel="noreferrer">
              Open thread
            </a>
          </section>
        )}

        <section className="sec">
          <h3>Times</h3>
          <p className="desc meta">
            Created {task.createdAt} · updated {task.updatedAt}
          </p>
        </section>
      </div>
    </aside>
  );
}
