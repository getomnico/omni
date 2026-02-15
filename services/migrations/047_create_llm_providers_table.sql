CREATE TABLE model_providers (
    id CHAR(26) PRIMARY KEY,
    name TEXT NOT NULL,
    provider_type TEXT NOT NULL CHECK (provider_type IN ('vllm', 'anthropic', 'bedrock', 'openai')),
    config JSONB NOT NULL DEFAULT '{}',
    is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TRIGGER update_model_providers_updated_at BEFORE UPDATE ON model_providers FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TABLE models (
    id CHAR(26) PRIMARY KEY,
    model_provider_id CHAR(26) NOT NULL REFERENCES model_providers(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    is_deleted BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX idx_models_single_default ON models (is_default) WHERE is_default = TRUE;
CREATE TRIGGER update_models_updated_at BEFORE UPDATE ON models FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

ALTER TABLE chats ADD COLUMN model_id CHAR(26) REFERENCES models(id) ON DELETE SET NULL;
