-- 088_add_configuration_table.sql
-- One key-value table for both org-wide settings and per-user preferences.
-- The `scope` column distinguishes the two; `user_id` is required for
-- user-scope rows and forbidden for global ones. Values are JSONB; the
-- consumer is responsible for stamping a version field if it ever needs
-- schema migration.

CREATE TABLE IF NOT EXISTS configuration (
    scope TEXT NOT NULL CHECK (scope IN ('global', 'user')),
    user_id CHAR(26) REFERENCES users(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT configuration_scope_user_consistency CHECK (
        (scope = 'global' AND user_id IS NULL)
        OR (scope = 'user' AND user_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS configuration_global_key_idx
    ON configuration (key) WHERE scope = 'global';

CREATE UNIQUE INDEX IF NOT EXISTS configuration_user_key_idx
    ON configuration (user_id, key) WHERE scope = 'user';

CREATE OR REPLACE TRIGGER set_configuration_updated_at
    BEFORE UPDATE ON configuration
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

INSERT INTO configuration (scope, user_id, key, value)
VALUES ('global', NULL, 'memory_mode_default', '{"value": "off"}')
ON CONFLICT DO NOTHING;
