ALTER TABLE sources
DROP CONSTRAINT IF EXISTS sources_source_type_check;

ALTER TABLE sources
ADD CONSTRAINT sources_source_type_check
CHECK (source_type IN ('google_drive', 'gmail', 'confluence', 'jira', 'slack',
  'github', 'local_files', 'web', 'notion', 'hubspot',
  'one_drive', 'share_point', 'outlook', 'outlook_calendar'));

ALTER TABLE service_credentials
DROP CONSTRAINT IF EXISTS service_credentials_provider_check;

ALTER TABLE service_credentials
ADD CONSTRAINT service_credentials_provider_check
CHECK (provider IN ('google', 'slack', 'atlassian', 'github', 'microsoft', 'notion', 'hubspot'));
