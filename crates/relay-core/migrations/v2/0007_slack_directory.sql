-- #33: metadata-only conversation directory, scan freshness and frozen windows.
-- No message bodies are stored in the directory; a frozen window keeps only the
-- read-only context captured at selection time.

CREATE TABLE slack_directory (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    members_json TEXT NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0,
    discovered INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);

CREATE TABLE slack_directory_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_success_at TEXT,
    last_cursor TEXT
);
INSERT INTO slack_directory_state (id, last_success_at, last_cursor) VALUES (1, NULL, NULL);

CREATE TABLE slack_window (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    selected_at TEXT NOT NULL,
    oldest TEXT NOT NULL,
    latest TEXT NOT NULL,
    messages_json TEXT NOT NULL,
    link TEXT NOT NULL,
    ready INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX slack_window_channel ON slack_window(channel_id);
