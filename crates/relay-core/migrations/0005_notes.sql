-- Notes: local scratch text the user can search, pin, hand to T3 or turn into a task.
-- `seq` is the rowid alias, so the FTS rowids stay stable across a VACUUM.
CREATE TABLE notes (
    seq        INTEGER PRIMARY KEY,
    id         TEXT NOT NULL UNIQUE,
    title      TEXT NOT NULL DEFAULT '',
    body       TEXT NOT NULL DEFAULT '',
    pinned     INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX notes_order ON notes (pinned DESC, updated_at DESC);

CREATE VIRTUAL TABLE notes_fts USING fts5(
    title,
    body,
    content = 'notes',
    content_rowid = 'seq'
);

CREATE TRIGGER notes_fts_insert AFTER INSERT ON notes BEGIN
    INSERT INTO notes_fts(rowid, title, body) VALUES (new.seq, new.title, new.body);
END;

CREATE TRIGGER notes_fts_delete AFTER DELETE ON notes BEGIN
    INSERT INTO notes_fts(notes_fts, rowid, title, body)
    VALUES ('delete', old.seq, old.title, old.body);
END;

CREATE TRIGGER notes_fts_update AFTER UPDATE OF title, body ON notes BEGIN
    INSERT INTO notes_fts(notes_fts, rowid, title, body)
    VALUES ('delete', old.seq, old.title, old.body);
    INSERT INTO notes_fts(rowid, title, body) VALUES (new.seq, new.title, new.body);
END;
