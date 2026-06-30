-- Durable conversation compaction summaries.
-- Raw chat_messages and agent_run_logs remain complete; these records only describe
-- the compacted prefix that should be sent to an LLM before the raw suffix.

CREATE TABLE IF NOT EXISTS compactions (
    id CHAR(26) PRIMARY KEY,
    target_type TEXT NOT NULL,
    chat_id CHAR(26) REFERENCES chats(id) ON DELETE CASCADE,
    agent_run_id CHAR(26) REFERENCES agent_runs(id) ON DELETE CASCADE,
    anchor_message_id VARCHAR(26) REFERENCES chat_messages(id) ON DELETE CASCADE,
    anchor_log_id CHAR(26) REFERENCES agent_run_logs(id) ON DELETE CASCADE,
    compacted_through_seq_num INTEGER NOT NULL,
    previous_compaction_id CHAR(26) REFERENCES compactions(id) ON DELETE SET NULL,
    summary TEXT NOT NULL,
    summary_message JSONB NOT NULL,
    estimated_input_tokens INTEGER,
    actual_input_tokens INTEGER,
    estimated_summary_tokens INTEGER,
    actual_summary_tokens INTEGER,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT compactions_target_type_check CHECK (target_type IN ('chat', 'agent_run')),
    CONSTRAINT compactions_target_anchor_check CHECK (
        (
            target_type = 'chat'
            AND chat_id IS NOT NULL
            AND anchor_message_id IS NOT NULL
            AND agent_run_id IS NULL
            AND anchor_log_id IS NULL
        )
        OR (
            target_type = 'agent_run'
            AND agent_run_id IS NOT NULL
            AND anchor_log_id IS NOT NULL
            AND chat_id IS NULL
            AND anchor_message_id IS NULL
        )
    ),
    CONSTRAINT compactions_seq_num_check CHECK (compacted_through_seq_num >= 0),
    CONSTRAINT compactions_estimated_input_tokens_check CHECK (estimated_input_tokens IS NULL OR estimated_input_tokens >= 0),
    CONSTRAINT compactions_actual_input_tokens_check CHECK (actual_input_tokens IS NULL OR actual_input_tokens >= 0),
    CONSTRAINT compactions_estimated_summary_tokens_check CHECK (estimated_summary_tokens IS NULL OR estimated_summary_tokens >= 0),
    CONSTRAINT compactions_actual_summary_tokens_check CHECK (actual_summary_tokens IS NULL OR actual_summary_tokens >= 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_compactions_anchor_message_unique
    ON compactions(anchor_message_id)
    WHERE anchor_message_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_compactions_anchor_log_unique
    ON compactions(anchor_log_id)
    WHERE anchor_log_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_compactions_chat_seq
    ON compactions(chat_id, compacted_through_seq_num DESC)
    WHERE target_type = 'chat';

CREATE INDEX IF NOT EXISTS idx_compactions_agent_run_seq
    ON compactions(agent_run_id, compacted_through_seq_num DESC)
    WHERE target_type = 'agent_run';

CREATE INDEX IF NOT EXISTS idx_compactions_previous
    ON compactions(previous_compaction_id)
    WHERE previous_compaction_id IS NOT NULL;
