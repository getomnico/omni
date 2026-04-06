-- API key authentication for programmatic access (agents, scripts, etc.)
CREATE TABLE api_keys (
    id CHAR(26) PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key_hash CHAR(64) NOT NULL,
    key_prefix VARCHAR(12) NOT NULL,
    name TEXT NOT NULL,
    allowed_sources JSONB DEFAULT NULL,
    scope TEXT NOT NULL DEFAULT 'public' CHECK (scope IN ('public', 'user', 'admin')),
    last_used_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX idx_api_keys_hash ON api_keys(key_hash);
CREATE INDEX idx_api_keys_user_id ON api_keys(user_id);

COMMENT ON COLUMN api_keys.allowed_sources IS 'JSON array of source_type strings this key can access. NULL = unrestricted.';
COMMENT ON COLUMN api_keys.scope IS 'Permission scope: public = only public documents, user = inherits creating user permissions, admin = all documents';
