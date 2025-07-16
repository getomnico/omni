-- Extend OAuth credentials to support web authentication (not just connectors)
-- Add a new table for user OAuth credentials separate from connector credentials

CREATE TABLE IF NOT EXISTS user_oauth_credentials (
    id CHAR(26) PRIMARY KEY,
    user_id CHAR(26) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider VARCHAR(50) NOT NULL,
    provider_user_id VARCHAR(255) NOT NULL, -- Google sub, Slack user ID, etc.
    access_token TEXT,
    refresh_token TEXT,
    token_type VARCHAR(50) DEFAULT 'Bearer',
    expires_at TIMESTAMPTZ,
    scopes TEXT[],
    profile_data JSONB NOT NULL DEFAULT '{}', -- Store profile info (name, email, avatar)
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT user_oauth_credentials_provider_check CHECK (provider IN ('google', 'slack', 'atlassian', 'github', 'microsoft')),
    CONSTRAINT user_oauth_credentials_unique UNIQUE (user_id, provider, provider_user_id)
);

CREATE INDEX IF NOT EXISTS idx_user_oauth_credentials_user_id ON user_oauth_credentials(user_id);
CREATE INDEX IF NOT EXISTS idx_user_oauth_credentials_provider ON user_oauth_credentials(provider);
CREATE INDEX IF NOT EXISTS idx_user_oauth_credentials_provider_user_id ON user_oauth_credentials(provider_user_id);
CREATE INDEX IF NOT EXISTS idx_user_oauth_credentials_expires_at ON user_oauth_credentials(expires_at);

DROP TRIGGER IF EXISTS update_user_oauth_credentials_updated_at ON user_oauth_credentials;
CREATE TRIGGER update_user_oauth_credentials_updated_at BEFORE UPDATE ON user_oauth_credentials
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

COMMENT ON TABLE user_oauth_credentials IS 'OAuth credentials for user authentication (separate from connector credentials)';
COMMENT ON COLUMN user_oauth_credentials.provider_user_id IS 'Provider-specific user ID (Google sub, Slack user ID, etc.)';
COMMENT ON COLUMN user_oauth_credentials.profile_data IS 'User profile information from OAuth provider (name, email, avatar, etc.)';
COMMENT ON COLUMN user_oauth_credentials.scopes IS 'Array of OAuth scopes granted for user authentication';