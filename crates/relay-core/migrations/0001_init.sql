-- Relay 0001: the local task graph. Mirrors docs/PLAN.md section 2.
-- Ids are ULIDs stored as TEXT; timestamps are RFC 3339 UTC TEXT.

CREATE TABLE projects (
    id                TEXT PRIMARY KEY,
    name              TEXT NOT NULL,
    slug              TEXT NOT NULL UNIQUE,
    repo_path         TEXT,
    color             TEXT,
    integrations_json TEXT NOT NULL DEFAULT '{}',
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);

CREATE TABLE project_statuses (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    category   TEXT NOT NULL CHECK (category IN (
        'backlog', 'todo', 'active', 'review', 'merged', 'deploying', 'done', 'blocked'
    )),
    position   INTEGER NOT NULL DEFAULT 0,
    color      TEXT
);

CREATE INDEX idx_project_statuses_project ON project_statuses(project_id, position);

CREATE TABLE tasks (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    status_id     TEXT NOT NULL REFERENCES project_statuses(id),
    title         TEXT NOT NULL,
    notes         TEXT NOT NULL DEFAULT '',
    priority      INTEGER,
    due_at        TEXT,
    position      INTEGER NOT NULL DEFAULT 0,
    source        TEXT NOT NULL DEFAULT 'local',
    rev           INTEGER NOT NULL DEFAULT 1,
    archived      INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    completed_at  TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX idx_tasks_project_status ON tasks(project_id, status_id);

CREATE TABLE claims (
    id               TEXT PRIMARY KEY,
    task_id          TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    holder_kind      TEXT NOT NULL,
    holder_name      TEXT NOT NULL,
    thread_ref       TEXT,
    session_ref      TEXT,
    lease_expires_at TEXT NOT NULL,
    released_at      TEXT,
    outcome          TEXT,
    created_at       TEXT NOT NULL
);

CREATE INDEX idx_claims_task_released ON claims(task_id, released_at);

CREATE TABLE task_changes (
    id            TEXT PRIMARY KEY,
    task_id       TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    claim_id      TEXT REFERENCES claims(id) ON DELETE SET NULL,
    kind          TEXT NOT NULL CHECK (kind IN (
        'requirement', 'new_task', 'change_scope', 'change_deadline',
        'change_priority', 'report_status', 'decision', 'no_change'
    )),
    body          TEXT NOT NULL,
    excerpt       TEXT,
    permalink     TEXT,
    source_kind   TEXT NOT NULL,
    source_id     TEXT,
    confidence    REAL,
    state         TEXT NOT NULL DEFAULT 'active' CHECK (state IN (
        'active', 'retracted', 'superseded'
    )),
    created_at    TEXT NOT NULL,
    acked_at      TEXT,
    superseded_by TEXT REFERENCES task_changes(id)
);

CREATE INDEX idx_task_changes_task_state ON task_changes(task_id, state);

CREATE TABLE task_events (
    id         TEXT PRIMARY KEY,
    task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    actor      TEXT,
    actor_kind TEXT,
    body       TEXT,
    data_json  TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);

CREATE INDEX idx_task_events_task_created ON task_events(task_id, created_at);

CREATE TABLE task_links (
    id             TEXT PRIMARY KEY,
    task_id        TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL,
    external_id    TEXT,
    url            TEXT NOT NULL,
    title          TEXT,
    state          TEXT,
    metadata_json  TEXT NOT NULL DEFAULT '{}',
    last_synced_at TEXT
);

CREATE INDEX idx_task_links_task_kind ON task_links(task_id, kind);

CREATE TABLE conversations (
    id              TEXT PRIMARY KEY,
    channel_id      TEXT NOT NULL,
    peer_id         TEXT NOT NULL,
    peer_name       TEXT,
    episode_no      INTEGER NOT NULL DEFAULT 1,
    state           TEXT NOT NULL DEFAULT 'open',
    opened_at       TEXT NOT NULL,
    last_message_at TEXT NOT NULL,
    closed_at       TEXT,
    jev_json        TEXT,
    summary         TEXT
);

CREATE INDEX idx_conversations_peer ON conversations(peer_id, opened_at);

CREATE TABLE messages (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ts              TEXT NOT NULL,
    author_id       TEXT NOT NULL,
    author_name     TEXT,
    direction       TEXT NOT NULL,
    text            TEXT NOT NULL,
    thread_ts       TEXT,
    permalink       TEXT,
    is_bot          INTEGER NOT NULL DEFAULT 0,
    latest_reply    TEXT,
    raw_json        TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX idx_messages_conversation_ts ON messages(conversation_id, ts);

CREATE TABLE analysis_jobs (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    project_id      TEXT REFERENCES projects(id) ON DELETE SET NULL,
    watermark       TEXT NOT NULL,
    kind            TEXT NOT NULL,
    state           TEXT NOT NULL DEFAULT 'pending',
    attempts        INTEGER NOT NULL DEFAULT 0,
    jev_json        TEXT,
    agent_session   TEXT,
    error           TEXT,
    created_at      TEXT NOT NULL,
    started_at      TEXT,
    finished_at     TEXT
);

CREATE INDEX idx_analysis_jobs_conversation_state ON analysis_jobs(conversation_id, state);

CREATE TABLE proposals (
    id               TEXT PRIMARY KEY,
    job_id           TEXT REFERENCES analysis_jobs(id) ON DELETE SET NULL,
    project_id       TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    task_id          TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    kind             TEXT NOT NULL,
    base_rev         INTEGER,
    patch_json       TEXT NOT NULL,
    rationale        TEXT,
    jev_json         TEXT,
    confidence       REAL,
    state            TEXT NOT NULL DEFAULT 'pending',
    decided_by       TEXT,
    decided_at       TEXT,
    applied_event_id TEXT REFERENCES task_events(id) ON DELETE SET NULL
);

CREATE INDEX idx_proposals_state ON proposals(state);

CREATE TABLE job_ops (
    id          TEXT PRIMARY KEY,
    job_id      TEXT NOT NULL REFERENCES analysis_jobs(id) ON DELETE CASCADE,
    op_index    INTEGER NOT NULL,
    tool        TEXT NOT NULL,
    result_json TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    UNIQUE (job_id, op_index)
);

CREATE TABLE sync_cursors (
    connector      TEXT NOT NULL,
    scope          TEXT NOT NULL,
    cursor         TEXT,
    last_synced_at TEXT,
    error          TEXT,
    PRIMARY KEY (connector, scope)
);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- FTS5 shortlist for task search and JEV candidates. External content, kept in
-- sync by triggers so a task write never needs a second statement.
CREATE VIRTUAL TABLE tasks_fts USING fts5(
    title,
    notes,
    content = 'tasks',
    content_rowid = 'rowid'
);

CREATE TRIGGER tasks_fts_insert AFTER INSERT ON tasks BEGIN
    INSERT INTO tasks_fts(rowid, title, notes) VALUES (new.rowid, new.title, new.notes);
END;

CREATE TRIGGER tasks_fts_delete AFTER DELETE ON tasks BEGIN
    INSERT INTO tasks_fts(tasks_fts, rowid, title, notes)
    VALUES ('delete', old.rowid, old.title, old.notes);
END;

CREATE TRIGGER tasks_fts_update AFTER UPDATE OF title, notes ON tasks BEGIN
    INSERT INTO tasks_fts(tasks_fts, rowid, title, notes)
    VALUES ('delete', old.rowid, old.title, old.notes);
    INSERT INTO tasks_fts(rowid, title, notes) VALUES (new.rowid, new.title, new.notes);
END;
