-- Add 'imap' to the sources.source_type check constraint (idempotent)
ALTER TABLE sources DROP CONSTRAINT IF EXISTS sources_source_type_check;
ALTER TABLE sources ADD CONSTRAINT sources_source_type_check
CHECK (source_type IN ('google_drive', 'gmail', 'confluence', 'jira', 'slack', 'notion', 'web', 'filesystem', 'fireflies', 'hubspot', 'sharepoint', 'onedrive', 'microsoft_teams', 'outlook', 'imap'));

-- Add 'imap' to the service_credentials.provider check constraint (idempotent)
ALTER TABLE service_credentials DROP CONSTRAINT IF EXISTS service_credentials_provider_check;
ALTER TABLE service_credentials ADD CONSTRAINT service_credentials_provider_check
CHECK (provider IN ('google', 'slack', 'atlassian', 'notion', 'fireflies', 'hubspot', 'microsoft', 'imap'));
