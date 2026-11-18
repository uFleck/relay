-- Analysis jobs (PLAN 3.2-3.3): one live job per episode per message watermark; a newer
-- watermark supersedes the older job. `job_ops` makes analyst writes idempotent by
-- <jobId>:<opIndex> so a timed-out run cannot create a second task or duplicate a
-- requirement.
CREATE TABLE analysis_jobs (
    id              TEXT PRIMARY KEY,
    episode_id      TEXT NOT NULL,
    watermark       TEXT NOT NULL,
    stage           TEXT NOT NULL,              -- screen | classify
    state           TEXT NOT NULL,              -- pending | running | pending_agent | done | failed | superseded | shadow
    project_id      TEXT,
    route           TEXT,                       -- ignore | review | change | new_task
    jev_json        TEXT,
    candidates_json TEXT NOT NULL DEFAULT '[]',
    summary         TEXT,
    error           TEXT,
    superseded_by   TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    started_at      TEXT,
    completed_at    TEXT
);
CREATE UNIQUE INDEX analysis_jobs_episode_watermark ON analysis_jobs (episode_id, watermark);
CREATE INDEX analysis_jobs_state ON analysis_jobs (state, created_at);
CREATE INDEX analysis_jobs_episode ON analysis_jobs (episode_id, created_at);

CREATE TABLE job_ops (
    id              TEXT PRIMARY KEY,
    job_id          TEXT NOT NULL REFERENCES analysis_jobs(id) ON DELETE CASCADE,
    op_index        INTEGER NOT NULL,
    kind            TEXT NOT NULL,              -- tasks_create | tasks_add_requirement | proposals_submit
    result_json     TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    UNIQUE (job_id, op_index)
);
CREATE INDEX job_ops_job ON job_ops (job_id, op_index);
