CREATE TABLE model_usage (
    id                    CHAR(26) PRIMARY KEY,
    user_id               CHAR(26) REFERENCES users(id),
    model_id              CHAR(26) NOT NULL REFERENCES models(id),
    model_name            TEXT NOT NULL,
    provider_type         TEXT NOT NULL,
    usage_type            TEXT NOT NULL,
    input_tokens          INTEGER NOT NULL DEFAULT 0,
    output_tokens         INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens     INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    chat_id               CHAR(26) REFERENCES chats(id),
    agent_run_id          CHAR(26),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

COMMENT ON COLUMN model_usage.user_id IS 'NULL for org-level agent runs that have no user context';
COMMENT ON COLUMN model_usage.model_name IS 'Denormalized model identifier so reports survive model deletion';
COMMENT ON COLUMN model_usage.usage_type IS 'One of: chat, title_generation, compaction, agent_run, agent_summary';

CREATE INDEX idx_model_usage_user_id ON model_usage(user_id) WHERE user_id IS NOT NULL;
CREATE INDEX idx_model_usage_model_id ON model_usage(model_id);
CREATE INDEX idx_model_usage_created_at ON model_usage(created_at);
CREATE INDEX idx_model_usage_chat_id ON model_usage(chat_id) WHERE chat_id IS NOT NULL;
