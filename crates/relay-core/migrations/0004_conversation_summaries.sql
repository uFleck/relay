-- Inbox row summaries: `conversations.summary` (0001) holds the text; `summary_through` is the
-- `last_message_at` it covers, so a newer message marks the summary stale.
ALTER TABLE conversations ADD COLUMN summary_through TEXT;
