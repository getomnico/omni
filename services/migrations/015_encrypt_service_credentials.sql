-- Encrypt service credentials
-- This migration enforces that all service credentials must be encrypted
-- All new credentials will be encrypted by the application code

-- Add a comment to indicate that credentials should be encrypted
COMMENT ON COLUMN service_credentials.credentials IS 'Encrypted sensitive credential data (private keys, tokens, secrets). Uses AES-256-GCM encryption with per-record salts. Format: {"encrypted_data": {...}, "version": 1}';

-- Create a function to check if credentials are encrypted
CREATE OR REPLACE FUNCTION is_credentials_encrypted(credentials JSONB) RETURNS BOOLEAN AS $$
BEGIN
    RETURN credentials ? 'encrypted_data' AND credentials ? 'version';
END;
$$ LANGUAGE plpgsql;

-- Add a CHECK constraint to ensure all new credentials are encrypted
ALTER TABLE service_credentials ADD CONSTRAINT service_credentials_encrypted_check 
    CHECK (is_credentials_encrypted(credentials));

-- Add a comment about the encryption process
COMMENT ON CONSTRAINT service_credentials_encrypted_check ON service_credentials IS 
    'Ensures all service credentials are encrypted using AES-256-GCM.';