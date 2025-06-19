-- Create sync runs table to track connector sync history
CREATE TABLE IF NOT EXISTS sync_runs (
    id CHAR(26) PRIMARY KEY,
    source_id CHAR(26) NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    sync_type VARCHAR(20) NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    status VARCHAR(20) NOT NULL DEFAULT 'running',
    files_processed INTEGER DEFAULT 0,
    files_updated INTEGER DEFAULT 0,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT sync_runs_sync_type_check CHECK (sync_type IN ('full', 'incremental')),
    CONSTRAINT sync_runs_status_check CHECK (status IN ('running', 'completed', 'failed'))
);

-- Index for finding latest sync runs by source
CREATE INDEX idx_sync_runs_source_completed ON sync_runs(source_id, completed_at DESC) WHERE status = 'completed';

-- Index for finding running syncs
CREATE INDEX idx_sync_runs_running ON sync_runs(source_id, status) WHERE status = 'running';

-- Index for monitoring and analytics
CREATE INDEX idx_sync_runs_source_type ON sync_runs(source_id, sync_type);

-- Trigger to update updated_at
DROP TRIGGER IF EXISTS update_sync_runs_updated_at ON sync_runs;
CREATE TRIGGER update_sync_runs_updated_at BEFORE UPDATE ON sync_runs
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();