-- Create response_feedback table to store user feedback on AI responses
CREATE TABLE response_feedback (
    id CHAR(26) PRIMARY KEY,
    message_id CHAR(26) NOT NULL REFERENCES chat_messages(id) ON DELETE CASCADE,
    user_id CHAR(26) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    feedback_type TEXT NOT NULL CHECK (feedback_type IN ('upvote', 'downvote')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(message_id, user_id)
);

-- Index for efficient queries by message
CREATE INDEX idx_response_feedback_message_id ON response_feedback(message_id);

-- Index for efficient queries by user
CREATE INDEX idx_response_feedback_user_id ON response_feedback(user_id);

-- Trigger to update updated_at timestamp
CREATE TRIGGER update_response_feedback_updated_at
    BEFORE UPDATE ON response_feedback
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
