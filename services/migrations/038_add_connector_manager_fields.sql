-- Add scheduling and state fields for connector manager
-- These fields support centralized sync scheduling and incremental sync state

-- Add scheduling fields to sources table
ALTER TABLE sources
ADD COLUMN IF NOT EXISTS sync_interval_seconds INTEGER DEFAULT 3600,
ADD COLUMN IF NOT EXISTS next_sync_at TIMESTAMPTZ,
ADD COLUMN IF NOT EXISTS connector_state JSONB;
-- TODO: Add schema validation for connector_state (out of scope for MVP)

-- Create index for scheduler to efficiently find sources due for sync
CREATE INDEX IF NOT EXISTS idx_sources_next_sync_at ON sources(next_sync_at)
WHERE is_active = true AND next_sync_at IS NOT NULL;

-- Add fields to sync_runs for manager tracking
ALTER TABLE sync_runs
ADD COLUMN IF NOT EXISTS trigger_type VARCHAR(20) DEFAULT 'manual',
ADD COLUMN IF NOT EXISTS queued_at TIMESTAMPTZ,
ADD COLUMN IF NOT EXISTS last_activity_at TIMESTAMPTZ;

-- Add constraint for trigger_type
ALTER TABLE sync_runs
ADD CONSTRAINT sync_runs_trigger_type_check
CHECK (trigger_type IN ('scheduled', 'manual', 'webhook'));

-- Index for finding stale syncs (running but no recent activity)
CREATE INDEX IF NOT EXISTS idx_sync_runs_stale_detection
ON sync_runs(last_activity_at)
WHERE status = 'running';

-- Add 'cancelled' to sync_runs status constraint
ALTER TABLE sync_runs DROP CONSTRAINT IF EXISTS sync_runs_status_check;
ALTER TABLE sync_runs ADD CONSTRAINT sync_runs_status_check
CHECK (status IN ('running', 'completed', 'failed', 'cancelled'));
