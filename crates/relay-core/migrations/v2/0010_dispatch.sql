-- Relay v2 0010: the intended T3 dispatch (#36).
-- The attempt is written before the child process starts. The partial unique index gives a Task
-- at most one unresolved attempt or one bound thread, so a restart or repeated Retry cannot start
-- a second thread while the first outcome is still uncertain. A known launch failure releases the
-- slot for one manual Retry. The thread id is unique across attempts so a link cannot steal a
-- thread already bound to another Task.

CREATE TABLE dispatch_attempts (
    attempt_id        TEXT PRIMARY KEY,
    task_id           TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    project_id        TEXT NOT NULL,
    project_selector  TEXT NOT NULL,
    t3_data_dir       TEXT NOT NULL,
    frozen_line       TEXT NOT NULL,
    brief_revision    INTEGER NOT NULL,
    base_ref          TEXT,
    title             TEXT NOT NULL,
    state             TEXT NOT NULL CHECK (state IN (
        'launching', 'launch_failed', 'reconcile_required', 'bound'
    )),
    thread_id         TEXT,
    run_id            TEXT,
    thread_url        TEXT,
    failure_reason    TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);

CREATE UNIQUE INDEX dispatch_one_open
    ON dispatch_attempts(task_id) WHERE state IN ('launching', 'reconcile_required', 'bound');
CREATE UNIQUE INDEX dispatch_thread_unique
    ON dispatch_attempts(thread_id) WHERE thread_id IS NOT NULL;
CREATE INDEX dispatch_state ON dispatch_attempts(state);
CREATE INDEX dispatch_task ON dispatch_attempts(task_id, created_at DESC);
