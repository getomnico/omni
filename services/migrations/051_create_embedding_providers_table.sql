CREATE TABLE embedding_providers (
    id CHAR(26) PRIMARY KEY,
    name TEXT NOT NULL,
    provider_type TEXT NOT NULL CHECK (provider_type IN ('local', 'jina', 'openai', 'cohere', 'bedrock')),
    config JSONB NOT NULL DEFAULT '{}',
    is_current BOOLEAN NOT NULL DEFAULT FALSE,
    is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Only one provider can be current at a time
CREATE UNIQUE INDEX idx_embedding_providers_single_current
    ON embedding_providers (is_current) WHERE is_current = TRUE AND is_deleted = FALSE;

CREATE TRIGGER set_updated_at BEFORE UPDATE ON embedding_providers
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

DROP TABLE IF EXISTS configuration;
