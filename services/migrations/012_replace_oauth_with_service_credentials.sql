-- Drop oauth_credentials table and create service_credentials table
-- This migration replaces OAuth-based authentication with service account credentials

-- First drop the oauth_credentials table
DROP TABLE IF EXISTS oauth_credentials CASCADE;

-- Create the new service_credentials table
CREATE TABLE service_credentials (
    id CHAR(26) PRIMARY KEY,
    source_id CHAR(26) NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    provider VARCHAR(50) NOT NULL,
    auth_type VARCHAR(50) NOT NULL,
    
    -- Common fields for all providers
    principal_email VARCHAR(255),
    
    -- JSON fields for provider-specific data
    -- credentials: Stores sensitive data like private keys, tokens, etc. (should be encrypted)
    credentials JSONB NOT NULL,
    -- config: Stores non-sensitive configuration like domains, scopes, project IDs, etc.
    config JSONB NOT NULL DEFAULT '{}',
    
    -- Metadata
    expires_at TIMESTAMPTZ,
    last_validated_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    
    -- Constraints
    CONSTRAINT service_credentials_provider_check CHECK (provider IN ('google', 'slack', 'atlassian', 'github', 'microsoft')),
    CONSTRAINT service_credentials_auth_type_check CHECK (auth_type IN ('jwt', 'api_key', 'basic_auth', 'bearer_token', 'bot_token'))
);

-- Create indexes
CREATE INDEX idx_service_credentials_source_id ON service_credentials(source_id);
CREATE INDEX idx_service_credentials_provider ON service_credentials(provider);
CREATE INDEX idx_service_credentials_expires_at ON service_credentials(expires_at);
CREATE UNIQUE INDEX idx_service_credentials_source_provider ON service_credentials(source_id, provider);

-- Add trigger for updated_at
DROP TRIGGER IF EXISTS update_service_credentials_updated_at ON service_credentials;
CREATE TRIGGER update_service_credentials_updated_at BEFORE UPDATE ON service_credentials
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- Add comments
COMMENT ON TABLE service_credentials IS 'Stores service account credentials for various providers';
COMMENT ON COLUMN service_credentials.provider IS 'Service provider (google, slack, atlassian, github, microsoft)';
COMMENT ON COLUMN service_credentials.auth_type IS 'Authentication type (jwt, api_key, basic_auth, bearer_token, bot_token)';
COMMENT ON COLUMN service_credentials.principal_email IS 'Service account email or principal user email';
COMMENT ON COLUMN service_credentials.credentials IS 'Encrypted sensitive credential data (private keys, tokens, secrets)';
COMMENT ON COLUMN service_credentials.config IS 'Non-sensitive configuration (domains, scopes, project IDs)';