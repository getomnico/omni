-- Create webhook_channels table to persist Google Drive webhook registrations
CREATE TABLE IF NOT EXISTS webhook_channels (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    channel_id TEXT NOT NULL UNIQUE,
    resource_id TEXT NOT NULL,
    resource_uri TEXT,
    webhook_url TEXT NOT NULL,
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- Index on source_id for efficient lookups
CREATE INDEX IF NOT EXISTS idx_webhook_channels_source_id ON webhook_channels(source_id);

-- Index on channel_id for webhook notification lookups
CREATE INDEX IF NOT EXISTS idx_webhook_channels_channel_id ON webhook_channels(channel_id);

-- Index on expires_at for renewal background task
CREATE INDEX IF NOT EXISTS idx_webhook_channels_expires_at ON webhook_channels(expires_at);

-- Add updated_at trigger
CREATE OR REPLACE FUNCTION update_webhook_channels_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trigger_update_webhook_channels_updated_at ON webhook_channels;
CREATE TRIGGER trigger_update_webhook_channels_updated_at
    BEFORE UPDATE ON webhook_channels
    FOR EACH ROW
    EXECUTE FUNCTION update_webhook_channels_updated_at();