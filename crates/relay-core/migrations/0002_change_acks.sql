-- Relay 0002: per-claim change acks.
--
-- `task_changes.acked_at` records the first ack, for the timeline. The claim that
-- acked lives here, because acks are keyed to a claim: a new claimant must not
-- inherit the previous claimant's acks (docs/PLAN.md section 2).
--
-- `claim_id` NULL is a human/direct-UI ack, kept explicit at the service boundary.

CREATE TABLE change_acks (
    change_id TEXT NOT NULL REFERENCES task_changes(id) ON DELETE CASCADE,
    claim_id  TEXT REFERENCES claims(id) ON DELETE CASCADE,
    acked_at  TEXT NOT NULL
);

-- SQLite treats NULLs as distinct in unique indexes; IFNULL makes the human ack
-- (NULL) idempotent too.
CREATE UNIQUE INDEX idx_change_acks_change_claim
    ON change_acks(change_id, IFNULL(claim_id, ''));

CREATE INDEX idx_change_acks_claim ON change_acks(claim_id);

-- Claim lookup on the hot path: active claim / expired reclaim.
CREATE INDEX idx_claims_task_lease ON claims(task_id, lease_expires_at);
