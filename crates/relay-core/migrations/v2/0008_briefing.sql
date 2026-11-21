-- Relay v2 0008: Briefing state for Act and Wait Tasks (#34).
-- One row per Task, keyed by the immutable Task id. The Brief revision and the
-- source signature let an adapter accept a model result only for the Task line
-- it was generated from. Approval is bound to the revision the user actually saw.

CREATE TABLE briefs (
    task_id                TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
    revision               INTEGER NOT NULL,
    source_signature       TEXT NOT NULL,
    status                 TEXT NOT NULL CHECK (status IN (
        'pending', 'waiting_reference', 'ready', 'failed', 'approved'
    )),
    content_json           TEXT,
    rendered               TEXT,
    link_only_refs         TEXT NOT NULL DEFAULT '[]',
    wait_approval_revision INTEGER,
    attempt_id             TEXT,
    rewrite_instruction    TEXT,
    failure_stage          TEXT,
    failure_reason         TEXT,
    created_at             TEXT NOT NULL,
    updated_at             TEXT NOT NULL
);

CREATE INDEX briefs_status ON briefs(status);
