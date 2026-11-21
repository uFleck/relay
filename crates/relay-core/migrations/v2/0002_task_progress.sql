-- Relay v2 0002: stored progress the Task views read and later slices (#35-#37) write.
-- All columns are nullable so a Task created before the producer lands still reads back.
-- grill_turn is only meaningful on a Grilling Task: whose turn it is, or whether the
-- closing Brief is waiting for the user's single approval.

ALTER TABLE tasks ADD COLUMN grill_turn TEXT CHECK (grill_turn IN ('agent', 'user_question', 'closing_approval'));
ALTER TABLE tasks ADD COLUMN pending_action TEXT;
ALTER TABLE tasks ADD COLUMN failure_stage TEXT;
ALTER TABLE tasks ADD COLUMN failure_reason TEXT;
ALTER TABLE tasks ADD COLUMN thread_url TEXT;

-- A Grilling Task starts on the agent's turn; the first question flips it to the user.
UPDATE tasks SET grill_turn = 'agent' WHERE agent_mode = 'grill' AND grill_turn IS NULL;
