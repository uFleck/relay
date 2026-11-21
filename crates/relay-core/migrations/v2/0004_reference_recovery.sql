-- Relay v2 0004: persistent recovery for unreadable References.
-- One row per canonical key, independent of Notes and the Import ledger, so a link shared by
-- two windows or many Notes still has exactly one schedule.

CREATE TABLE ref_recovery (
    key            TEXT PRIMARY KEY,
    kind           TEXT NOT NULL,
    target_id      TEXT NOT NULL,
    canonical_url  TEXT NOT NULL,
    provider       TEXT NOT NULL,
    attempts       INTEGER NOT NULL DEFAULT 0,
    next_due_at    TEXT NOT NULL,
    next_due_ms    INTEGER NOT NULL,
    reason         TEXT,
    paused         INTEGER NOT NULL DEFAULT 0,
    paused_config  TEXT,
    updated_at     TEXT NOT NULL
);

CREATE INDEX ref_recovery_due ON ref_recovery(next_due_ms) WHERE paused = 0;
