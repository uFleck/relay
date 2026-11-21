-- Relay v2 0005: live refresh schedule for imported References.
-- Separate from ref_recovery: a successful live read reschedules at the provider cadence
-- (GitHub 3 minutes, ClickUp 10 minutes) instead of deleting the row. It is keyed by the
-- canonical Reference identity and never references a Note, so it survives Note deletion.

CREATE TABLE import_refresh (
    key           TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    target_id     TEXT NOT NULL,
    canonical_url TEXT NOT NULL,
    provider      TEXT NOT NULL,
    account_id    TEXT,
    attempts      INTEGER NOT NULL DEFAULT 0,
    next_due_at   TEXT NOT NULL,
    next_due_ms   INTEGER NOT NULL,
    reason        TEXT,
    updated_at    TEXT NOT NULL
);

CREATE INDEX import_refresh_due ON import_refresh(next_due_ms);
