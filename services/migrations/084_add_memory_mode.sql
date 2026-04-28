-- 084_add_memory_mode.sql
-- Memory feature: org-wide default in `configuration` and a per-user
-- override in a new `user_preferences` table.

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

INSERT INTO configuration (key, value)
VALUES ('memory_mode_default', '{"mode": "off"}')
ON CONFLICT DO NOTHING;

CREATE TABLE IF NOT EXISTS user_preferences (
    user_id CHAR(26) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, key)
);

CREATE OR REPLACE TRIGGER set_user_preferences_updated_at
    BEFORE UPDATE ON user_preferences
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- Self-heal dev DBs that ran an earlier version of this migration which
-- added users.memory_mode directly: backfill survivors then drop the column.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'users'
          AND column_name = 'memory_mode'
    ) THEN
        EXECUTE $sql$
            INSERT INTO user_preferences (user_id, key, value)
            SELECT id, 'memory_mode', to_jsonb(memory_mode)
            FROM users WHERE memory_mode IS NOT NULL
            ON CONFLICT DO NOTHING
        $sql$;
        ALTER TABLE users DROP COLUMN memory_mode;
    END IF;
END $$;
