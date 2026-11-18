-- Slack DM capture (PLAN 3.1). Messages are local-only; retention deletes rows but a
-- task change keeps its own excerpt and permalink.
CREATE TABLE slack_peers (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL UNIQUE,
    name            TEXT,
    is_bot          INTEGER NOT NULL DEFAULT 0,
    excluded        INTEGER NOT NULL DEFAULT 0,
    last_message_at TEXT,
    created_at      TEXT NOT NULL
);

CREATE TABLE slack_episodes (
    id              TEXT PRIMARY KEY,
    peer_id         TEXT NOT NULL REFERENCES slack_peers(id) ON DELETE CASCADE,
    state           TEXT NOT NULL,              -- open | waiting | closed | analyzed | needs_review
    started_at      TEXT NOT NULL,
    last_message_at TEXT NOT NULL,
    closed_at       TEXT,
    summary         TEXT,
    jev_json        TEXT,
    created_at      TEXT NOT NULL
);
CREATE INDEX slack_episodes_peer ON slack_episodes (peer_id, last_message_at DESC);
CREATE INDEX slack_episodes_state ON slack_episodes (state, last_message_at DESC);

CREATE TABLE slack_messages (
    id              TEXT PRIMARY KEY,
    episode_id      TEXT NOT NULL REFERENCES slack_episodes(id) ON DELETE CASCADE,
    peer_id         TEXT NOT NULL,
    channel_id      TEXT NOT NULL,
    ts              TEXT NOT NULL,
    thread_ts       TEXT,
    direction       TEXT NOT NULL,              -- in | out
    user_id         TEXT,
    text            TEXT NOT NULL,
    permalink       TEXT,
    edited_at       TEXT,
    deleted         INTEGER NOT NULL DEFAULT 0,
    historical      INTEGER NOT NULL DEFAULT 0, -- 14-day backfill, shadow mode
    raw_json        TEXT,
    created_at      TEXT NOT NULL,
    UNIQUE (channel_id, ts)
);
CREATE INDEX slack_messages_episode ON slack_messages (episode_id, ts);

-- One DM channel per peer. `last_ts` is the catch-up watermark for that channel.
CREATE TABLE slack_channels (
    channel_id      TEXT PRIMARY KEY,
    peer_id         TEXT NOT NULL REFERENCES slack_peers(id) ON DELETE CASCADE,
    last_ts         TEXT,
    last_synced_at  TEXT
);
