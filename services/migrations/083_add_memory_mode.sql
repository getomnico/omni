-- 083_add_memory_mode.sql
-- Add per-user memory mode preference and org-wide default.

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS memory_mode TEXT
    CHECK (memory_mode IN ('off', 'chat', 'full'));

-- The configuration table was dropped unconditionally in migration 051.
-- Recreate it here (schema from migration 032; trigger renamed to match later conventions).
CREATE TABLE IF NOT EXISTS configuration (
    key TEXT PRIMARY KEY,
    value JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE OR REPLACE TRIGGER set_configuration_updated_at
    BEFORE UPDATE ON configuration
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- Insert the factory default row if it doesn't exist yet.
INSERT INTO configuration (key, value)
VALUES ('memory_mode_default', '{"mode": "off"}')
ON CONFLICT DO NOTHING;
