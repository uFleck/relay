-- Relay v2 0001: the fresh store. No v1 tables, no v1-to-v2 user_version migration.
-- Ids are ULIDs as TEXT; timestamps are fixed-width RFC 3339 UTC TEXT.

CREATE TABLE projects (
    seq        INTEGER PRIMARY KEY,
    id         TEXT NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    slug       TEXT NOT NULL UNIQUE,
    repo_path  TEXT,
    owner_repo TEXT,
    t3_target  TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- A repo may be registered once, case-insensitively, or left unset.
CREATE UNIQUE INDEX projects_owner_repo ON projects(owner_repo) WHERE owner_repo IS NOT NULL;

CREATE TABLE notes (
    seq        INTEGER PRIMARY KEY,
    id         TEXT NOT NULL UNIQUE,
    rev        INTEGER NOT NULL,
    source     TEXT NOT NULL,
    body       TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE note_lines (
    id         TEXT PRIMARY KEY,
    note_id    TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    text       TEXT NOT NULL,
    -- Active lines own a unique position in the note. A removed line keeps its row
    -- with a null position so a Task created from it stays addressable.
    position   INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX note_lines_active_position
    ON note_lines(note_id, position) WHERE position IS NOT NULL;
CREATE INDEX note_lines_note ON note_lines(note_id, position);

CREATE TABLE tasks (
    seq            INTEGER PRIMARY KEY,
    id             TEXT NOT NULL UNIQUE,
    source_line_id TEXT NOT NULL UNIQUE REFERENCES note_lines(id) ON DELETE CASCADE,
    original_line  TEXT NOT NULL,
    agent_mode     TEXT NOT NULL CHECK (agent_mode IN ('act', 'wait', 'grill')),
    status         TEXT NOT NULL CHECK (status IN (
        'briefing', 'grilling', 'needs_project', 'awaiting_approval', 'dispatched',
        'needs_answer', 'returned', 'failed', 'done', 'cancelled'
    )),
    project_id     TEXT REFERENCES projects(id) ON DELETE SET NULL,
    brief          TEXT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE INDEX tasks_status ON tasks(status);
CREATE INDEX tasks_project ON tasks(project_id);

-- A Reference is keyed by kind plus canonical target id, not by its display text.
CREATE TABLE ref_targets (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    target_id     TEXT NOT NULL,
    canonical_url TEXT NOT NULL,
    snapshot_json TEXT,
    created_at    TEXT NOT NULL,
    UNIQUE (kind, target_id)
);

CREATE TABLE line_refs (
    line_id  TEXT NOT NULL REFERENCES note_lines(id) ON DELETE CASCADE,
    ref_id   TEXT NOT NULL REFERENCES ref_targets(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    PRIMARY KEY (line_id, ref_id)
);

-- Copied when a Task is created. Later Note edits never rewrite this membership.
CREATE TABLE task_refs (
    task_id  TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    ref_id   TEXT NOT NULL REFERENCES ref_targets(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    PRIMARY KEY (task_id, ref_id)
);

-- A create submission key is recorded once, so a replay returns the original Note.
CREATE TABLE submission_keys (
    submission_key TEXT PRIMARY KEY,
    note_id        TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    created_at     TEXT NOT NULL
);

-- The ledger key is independent of the Note's lifetime. `note_id` is deliberately
-- not a foreign key: deleting the fixture Note must not release the key.
CREATE TABLE import_ledger (
    source_kind TEXT NOT NULL,
    target_id   TEXT NOT NULL,
    note_id     TEXT,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (source_kind, target_id)
);

-- Typed v2 defaults. Import is off and its scope is unset on first launch.
CREATE TABLE settings (
    id                    INTEGER PRIMARY KEY CHECK (id = 1),
    import_clickup_enabled INTEGER NOT NULL DEFAULT 0 CHECK (import_clickup_enabled IN (0, 1)),
    import_github_enabled  INTEGER NOT NULL DEFAULT 0 CHECK (import_github_enabled IN (0, 1)),
    import_scope           TEXT,
    updated_at             TEXT NOT NULL
);

CREATE VIRTUAL TABLE notes_fts USING fts5(body, content = 'notes', content_rowid = 'seq');

CREATE TRIGGER notes_fts_insert AFTER INSERT ON notes BEGIN
    INSERT INTO notes_fts(rowid, body) VALUES (new.seq, new.body);
END;
CREATE TRIGGER notes_fts_delete AFTER DELETE ON notes BEGIN
    INSERT INTO notes_fts(notes_fts, rowid, body) VALUES ('delete', old.seq, old.body);
END;
CREATE TRIGGER notes_fts_update AFTER UPDATE OF body ON notes BEGIN
    INSERT INTO notes_fts(notes_fts, rowid, body) VALUES ('delete', old.seq, old.body);
    INSERT INTO notes_fts(rowid, body) VALUES (new.seq, new.body);
END;

CREATE VIRTUAL TABLE tasks_fts USING fts5(original_line, content = 'tasks', content_rowid = 'seq');

CREATE TRIGGER tasks_fts_insert AFTER INSERT ON tasks BEGIN
    INSERT INTO tasks_fts(rowid, original_line) VALUES (new.seq, new.original_line);
END;
CREATE TRIGGER tasks_fts_delete AFTER DELETE ON tasks BEGIN
    INSERT INTO tasks_fts(tasks_fts, rowid, original_line)
    VALUES ('delete', old.seq, old.original_line);
END;
CREATE TRIGGER tasks_fts_update AFTER UPDATE OF original_line ON tasks BEGIN
    INSERT INTO tasks_fts(tasks_fts, rowid, original_line)
    VALUES ('delete', old.seq, old.original_line);
    INSERT INTO tasks_fts(rowid, original_line) VALUES (new.seq, new.original_line);
END;
