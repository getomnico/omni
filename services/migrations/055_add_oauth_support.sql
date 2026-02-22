-- Add 'oauth' to auth_type constraint on service_credentials
ALTER TABLE service_credentials
  DROP CONSTRAINT IF EXISTS service_credentials_auth_type_check;
ALTER TABLE service_credentials
  ADD CONSTRAINT service_credentials_auth_type_check
  CHECK (auth_type IN ('jwt', 'api_key', 'basic_auth', 'bearer_token', 'bot_token', 'oauth'));

-- Create connector_configs table for connector-level configuration (e.g. OAuth app credentials)
CREATE TABLE connector_configs (
    provider TEXT PRIMARY KEY,
    config JSONB NOT NULL DEFAULT '{}',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_by TEXT REFERENCES users(id)
);
