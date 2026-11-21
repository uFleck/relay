-- Relay v2 0006: on-demand Slack thread cache. Separate from the v1 Slack tables and from
-- `ref_targets` (which holds only the read-only Lens snapshot). Keyed by channel plus the root
-- timestamp, so a parent permalink and a reply permalink with the same `thread_ts` share one hit.

CREATE TABLE slack_thread_cache (
    channel_id   TEXT NOT NULL,
    thread_ts    TEXT NOT NULL,
    parent_json  TEXT,
    replies_json TEXT NOT NULL,
    fetched_at   TEXT NOT NULL,
    PRIMARY KEY (channel_id, thread_ts)
);
