CREATE TABLE IF NOT EXISTS sources (
    id CHAR(26) PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    source_type VARCHAR(50) NOT NULL,
    config JSONB NOT NULL DEFAULT '{}',
    oauth_credentials JSONB,
    is_active BOOLEAN NOT NULL DEFAULT true,
    last_sync_at TIMESTAMPTZ,
    sync_status VARCHAR(50) DEFAULT 'pending',
    sync_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_by CHAR(26) NOT NULL REFERENCES users(id),
    CONSTRAINT sources_sync_status_check CHECK (sync_status IN ('pending', 'syncing', 'completed', 'failed'))
);

CREATE INDEX IF NOT EXISTS idx_sources_source_type ON sources(source_type);
CREATE INDEX IF NOT EXISTS idx_sources_is_active ON sources(is_active);
CREATE INDEX IF NOT EXISTS idx_sources_sync_status ON sources(sync_status);
CREATE INDEX IF NOT EXISTS idx_sources_created_by ON sources(created_by);

DROP TRIGGER IF EXISTS update_sources_updated_at ON sources;
CREATE TRIGGER update_sources_updated_at BEFORE UPDATE ON sources
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();