ALTER TABLE sources DROP CONSTRAINT IF EXISTS sources_source_type_check;
ALTER TABLE sources ADD CONSTRAINT sources_source_type_check
CHECK (source_type IN (
  'google_drive',
  'gmail',
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
  'ms_teams'
));
