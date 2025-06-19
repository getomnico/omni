-- Create connector events queue table for reliable message processing
CREATE TABLE IF NOT EXISTS connector_events_queue (
    id CHAR(26) PRIMARY KEY,
    sync_run_id CHAR(26) NOT NULL,
    source_id CHAR(26) NOT NULL,
    event_type VARCHAR(50) NOT NULL,
    payload JSONB NOT NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'pending',
    retry_count INTEGER DEFAULT 0,
    max_retries INTEGER DEFAULT 3,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    processed_at TIMESTAMPTZ,
    error_message TEXT,
    CONSTRAINT valid_status CHECK (status IN ('pending', 'processing', 'completed', 'failed', 'dead_letter'))
);

-- Index for efficient polling of pending events
CREATE INDEX idx_queue_status_created ON connector_events_queue(status, created_at) WHERE status = 'pending';

-- Index for querying events by source
CREATE INDEX idx_queue_source_id ON connector_events_queue(source_id);

-- Index for monitoring processing status
CREATE INDEX idx_queue_status ON connector_events_queue(status);

-- Index for finding failed events that need retry
CREATE INDEX idx_queue_retry ON connector_events_queue(status, retry_count) WHERE status = 'failed' AND retry_count < max_retries;

-- Index for querying events by sync run
CREATE INDEX idx_queue_sync_run_id ON connector_events_queue(sync_run_id);