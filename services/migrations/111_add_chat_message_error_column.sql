-- Persist chat stream errors on the failed assistant turn's row so they
-- survive page reloads. The `message` column stays a pure Anthropic
-- MessageParam; the error lives in its own typed JSONB column.
ALTER TABLE chat_messages ADD COLUMN error jsonb;
