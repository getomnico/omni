ALTER TABLE tool_approvals
    ADD COLUMN IF NOT EXISTS approval_type TEXT NOT NULL DEFAULT 'approval',
    ADD COLUMN IF NOT EXISTS tool_call_id TEXT,
    ADD COLUMN IF NOT EXISTS provider TEXT,
    ADD COLUMN IF NOT EXISTS oauth_start_url TEXT;

ALTER TABLE tool_approvals
    ADD CONSTRAINT tool_approvals_approval_type_check
    CHECK (approval_type IN ('approval', 'oauth'));

CREATE INDEX IF NOT EXISTS idx_tool_approvals_chat_type_status
    ON tool_approvals(chat_id, approval_type, status);
