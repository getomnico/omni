ALTER TABLE chat_messages
    ADD COLUMN terminal_reason TEXT,
    ADD COLUMN continued_at TIMESTAMPTZ,
    ADD COLUMN continuation_message_id VARCHAR(26);

ALTER TABLE chat_messages
    ADD CONSTRAINT chat_messages_terminal_reason_check
    CHECK (terminal_reason IS NULL OR terminal_reason = 'iteration_limit');
