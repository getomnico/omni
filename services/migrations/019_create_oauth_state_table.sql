-- OAuth state management for web authentication flows
-- This is separate from connector OAuth credentials and handles CSRF protection

CREATE TABLE IF NOT EXISTS oauth_state (
    id CHAR(26) PRIMARY KEY,
    state_token VARCHAR(255) NOT NULL UNIQUE,
    provider VARCHAR(50) NOT NULL,
    redirect_uri TEXT,
    nonce VARCHAR(255),
    code_verifier VARCHAR(255), -- For PKCE if needed
    user_id CHAR(26) REFERENCES users(id) ON DELETE CASCADE, -- Optional: for linking flow
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    metadata JSONB NOT NULL DEFAULT '{}',
    CONSTRAINT oauth_state_provider_check CHECK (provider IN ('google', 'slack', 'atlassian', 'github', 'microsoft'))
);

CREATE INDEX IF NOT EXISTS idx_oauth_state_token ON oauth_state(state_token);
CREATE INDEX IF NOT EXISTS idx_oauth_state_provider ON oauth_state(provider);
CREATE INDEX IF NOT EXISTS idx_oauth_state_expires_at ON oauth_state(expires_at);
CREATE INDEX IF NOT EXISTS idx_oauth_state_user_id ON oauth_state(user_id);

-- Clean up expired state tokens periodically
CREATE OR REPLACE FUNCTION cleanup_expired_oauth_state() RETURNS void AS $$
BEGIN
    DELETE FROM oauth_state WHERE expires_at < NOW();
END;
$$ LANGUAGE plpgsql;

COMMENT ON TABLE oauth_state IS 'Temporary OAuth state tokens for CSRF protection during web authentication flows';
COMMENT ON COLUMN oauth_state.state_token IS 'Random token sent to OAuth provider for CSRF protection';
COMMENT ON COLUMN oauth_state.provider IS 'OAuth provider (google, slack, atlassian, github, microsoft)';
COMMENT ON COLUMN oauth_state.redirect_uri IS 'URI to redirect to after successful OAuth flow';
COMMENT ON COLUMN oauth_state.nonce IS 'OpenID Connect nonce for additional security';
COMMENT ON COLUMN oauth_state.code_verifier IS 'PKCE code verifier for enhanced security';
COMMENT ON COLUMN oauth_state.user_id IS 'Optional user ID for account linking scenarios';
COMMENT ON COLUMN oauth_state.metadata IS 'Additional provider-specific data';