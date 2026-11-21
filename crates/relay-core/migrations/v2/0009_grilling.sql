-- Relay v2 0009: persisted Grilling exchange for ?? Tasks (#35).
-- One row per Task line generation. A pre-dispatch Task-line edit starts a new
-- revision, so the earlier transcript stays with the Task instead of being lost.
-- The final Brief itself lives in briefs; the second gate there is the closing Go.

CREATE TABLE grills (
    task_id           TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    revision          INTEGER NOT NULL,
    source_signature  TEXT NOT NULL,
    status            TEXT NOT NULL CHECK (status IN (
        'pending', 'question', 'brief', 'failed', 'approved', 'cancelled'
    )),
    attempt_id        TEXT,
    question          TEXT,
    transcript        TEXT NOT NULL DEFAULT '[]',
    reference_ids     TEXT NOT NULL DEFAULT '[]',
    failure_stage     TEXT,
    failure_reason    TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    PRIMARY KEY (task_id, revision)
);

CREATE INDEX grills_task ON grills(task_id, revision DESC);
