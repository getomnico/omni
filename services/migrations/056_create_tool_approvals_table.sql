-- Tool approvals table for tracking user approval of write actions
CREATE TABLE IF NOT EXISTS tool_approvals (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL,
    tool_input JSONB NOT NULL,
    source_id TEXT,
    source_type TEXT,
    status TEXT NOT NULL DEFAULT 'pending',  -- pending, approved, denied, expired
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at TIMESTAMPTZ,
    resolved_by TEXT REFERENCES users(id)
);

CREATE INDEX IF NOT EXISTS idx_tool_approvals_chat_id ON tool_approvals(chat_id);
CREATE INDEX IF NOT EXISTS idx_tool_approvals_status ON tool_approvals(status) WHERE status = 'pending';
