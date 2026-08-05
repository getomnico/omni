-- Add Windshift as a valid source_type and service_credentials provider.
ALTER TABLE sources DROP CONSTRAINT IF EXISTS sources_source_type_check;
ALTER TABLE sources ADD CONSTRAINT sources_source_type_check
CHECK (
  (integration_type = 'connector' AND source_type IN (
    'google_drive',
    'gmail',
    'google_chat',
    'confluence',
    'jira',
    'slack',
    'notion',
    'web',
    'github',
    'local_files',
    'file_system',
    'fireflies',
    'hubspot',
    'one_drive',
    'share_point',
    'outlook',
    'outlook_calendar',
    'imap',
    'clickup',
    'linear',
    'ms_teams',
    'paperless_ngx',
    'nextcloud',
    'google_ads',
    'darwinbox',
    'windshift'
  ))
  OR
  (integration_type = 'remote_mcp' AND source_type ~ '^[a-z][a-z0-9_-]{1,49}$')
);

ALTER TABLE service_credentials DROP CONSTRAINT IF EXISTS service_credentials_provider_check;
ALTER TABLE service_credentials ADD CONSTRAINT service_credentials_provider_check
CHECK (provider IN (
  'google',
  'slack',
  'atlassian',
  'github',
  'notion',
  'fireflies',
  'hubspot',
  'microsoft',
  'imap',
  'clickup',
  'linear',
  'paperless_ngx',
  'nextcloud',
  'google_ads',
  'darwinbox',
  'remote_mcp',
  'windshift'
));
