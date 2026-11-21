-- Relay v2 0002: OpenCode's model selection. The provider key stays in the v2
-- Credential Manager service; only the non-secret `provider/model` string lives here.

ALTER TABLE settings ADD COLUMN opencode_model TEXT;
