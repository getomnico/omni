-- Soft-delete on chats. We never destroy chat rows because model_usage rows
-- are FK'd to chats(id) and we want token-usage history preserved forever,
-- with the chat link intact. The DELETE handler now flips is_deleted; every
-- read path filters is_deleted = FALSE.

ALTER TABLE chats ADD COLUMN is_deleted BOOLEAN NOT NULL DEFAULT FALSE;

-- Most reads filter (user_id, updated_at) for non-deleted chats; a partial
-- index keeps that path fast and excludes soft-deleted rows from the index.
CREATE INDEX idx_chats_user_active ON chats(user_id, updated_at DESC) WHERE is_deleted = FALSE;
