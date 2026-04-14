-- User-owned ad-hoc file uploads, referenced from chat messages via
-- {"type": "document"|"image", "source": {"type": "omni_upload", "upload_id": ...}} blocks.
-- Not bound to a chat: a single upload can be referenced from any number of messages/chats.

CREATE TABLE IF NOT EXISTS uploads (
    id CHAR(26) PRIMARY KEY,
    user_id CHAR(26) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    content_id CHAR(26) NOT NULL REFERENCES content_blobs(id) ON DELETE RESTRICT,
    filename TEXT NOT NULL,
    content_type VARCHAR(255) NOT NULL,
    size_bytes BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_uploads_user_id ON uploads(user_id);
CREATE INDEX IF NOT EXISTS idx_uploads_content_id ON uploads(content_id);
CREATE INDEX IF NOT EXISTS idx_uploads_created_at ON uploads(created_at DESC);
