ALTER TABLE sources
DROP CONSTRAINT IF EXISTS sources_source_type_check;

ALTER TABLE sources
ADD CONSTRAINT sources_source_type_check
CHECK (source_type IN ('google_drive', 'gmail', 'confluence', 'jira', 'slack', 'github', 'local_files', 'web', 'notion', 'microsoft'));
