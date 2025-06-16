-- Standardize source_type values and add CHECK constraint
-- Add CHECK constraint to enforce valid source_type values
ALTER TABLE sources 
ADD CONSTRAINT sources_source_type_check 
CHECK (source_type IN ('google_drive', 'gmail', 'confluence', 'jira', 'slack', 'github', 'local_files'));