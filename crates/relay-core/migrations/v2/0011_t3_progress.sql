-- Relay v2 0011: observed T3 progress and continuation operations (#37).
-- One progress row per bound dispatch attempt keeps the last good observation, so a failed read
-- never erases what Relay already knows. revision fences a read that was in flight when the Task
-- was closed or a newer observation committed. An operation reserves a Follow-up, Retry or resend
-- before the CLI runs, so a lost response can be reconciled instead of replaying the message.

CREATE TABLE t3_progress (
    attempt_id           TEXT PRIMARY KEY REFERENCES dispatch_attempts(attempt_id) ON DELETE CASCADE,
    task_id              TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    thread_id            TEXT NOT NULL,
    run_id               TEXT,
    run_count            INTEGER NOT NULL DEFAULT 0,
    status               TEXT,
    pending_request_count INTEGER NOT NULL DEFAULT 0,
    branch               TEXT,
    worktree_path        TEXT,
    link                 TEXT,
    reply                TEXT,
    error                TEXT,
    stale_reason         TEXT,
    revision             INTEGER NOT NULL DEFAULT 0,
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL
);
CREATE INDEX t3_progress_task ON t3_progress(task_id);

CREATE TABLE t3_operations (
    operation_id        TEXT PRIMARY KEY,
    attempt_id          TEXT NOT NULL REFERENCES dispatch_attempts(attempt_id) ON DELETE CASCADE,
    task_id             TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    thread_id           TEXT NOT NULL,
    kind                TEXT NOT NULL CHECK (kind IN ('followup', 'retry', 'resend')),
    state               TEXT NOT NULL CHECK (state IN (
        'reserved', 'acknowledged', 'needs_confirmation', 'failed', 'stale'
    )),
    message             TEXT NOT NULL,
    baseline_run_id     TEXT,
    baseline_run_count  INTEGER NOT NULL DEFAULT 0,
    run_id              TEXT,
    delivery_state      TEXT,
    failure_reason      TEXT,
    revision            INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);
CREATE INDEX t3_operations_task ON t3_operations(task_id, created_at DESC);
CREATE UNIQUE INDEX t3_operations_open
    ON t3_operations(task_id) WHERE state IN ('reserved', 'acknowledged', 'needs_confirmation');
