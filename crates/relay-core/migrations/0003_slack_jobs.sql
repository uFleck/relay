-- Slack capture and the analysis-job fields P3 needs. The 0001 schema already has
-- conversations, messages, analysis_jobs and job_ops; this migration extends them
-- instead of creating parallel tables.
CREATE TABLE slack_peers (
    id         TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL UNIQUE,
    name       TEXT,
    is_bot     INTEGER NOT NULL DEFAULT 0,
    excluded   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

ALTER TABLE messages ADD COLUMN edited_at TEXT;
ALTER TABLE messages ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN historical INTEGER NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN created_at TEXT NOT NULL DEFAULT '';
CREATE INDEX idx_messages_ts ON messages(ts);

ALTER TABLE analysis_jobs ADD COLUMN route TEXT;
ALTER TABLE analysis_jobs ADD COLUMN candidates_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE analysis_jobs ADD COLUMN summary TEXT;
ALTER TABLE analysis_jobs ADD COLUMN updated_at TEXT;
ALTER TABLE analysis_jobs ADD COLUMN superseded_by TEXT;
CREATE UNIQUE INDEX idx_analysis_jobs_conversation_watermark
    ON analysis_jobs(conversation_id, watermark);
CREATE INDEX idx_analysis_jobs_state_created ON analysis_jobs(state, created_at);
