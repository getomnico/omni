-- Add support for UI-created remote MCP source rows without overloading
-- source_type. source_type remains the app/provider slug; integration_type
-- distinguishes native connector-backed rows from remote MCP rows.

-- 1. Integration discriminator on sources. Existing rows are native connectors.
ALTER TABLE sources ADD COLUMN integration_type TEXT NOT NULL DEFAULT 'connector';
ALTER TABLE sources ADD CONSTRAINT sources_integration_type_check
    CHECK (integration_type IN ('connector', 'remote_mcp'));

CREATE INDEX idx_sources_integration_type ON sources (integration_type);

-- 2. Add the generic credential provider used for credentials belonging to
-- UI-created remote MCP integrations. Existing auth_type values are reused.
ALTER TABLE service_credentials DROP CONSTRAINT IF EXISTS service_credentials_provider_check;
ALTER TABLE service_credentials ADD CONSTRAINT service_credentials_provider_check
CHECK (provider IN (
  'google', 'slack', 'atlassian', 'github', 'notion', 'fireflies',
  'hubspot', 'microsoft', 'imap', 'clickup', 'linear', 'paperless_ngx',
  'nextcloud', 'google_ads', 'darwinbox',
  'remote_mcp'
));

-- 3. Keep source_type's existing "app name" meaning. Native connector rows
-- remain constrained to known SourceType values, while remote MCP rows may use
-- a validated app/server slug supplied during setup.
ALTER TABLE sources DROP CONSTRAINT IF EXISTS sources_source_type_check;
ALTER TABLE sources ADD CONSTRAINT sources_source_type_check CHECK (
    (integration_type = 'connector' AND source_type IN (
      'google_drive', 'gmail', 'google_chat', 'confluence', 'jira', 'slack',
      'notion', 'web', 'github', 'local_files', 'file_system', 'fireflies',
      'hubspot', 'one_drive', 'share_point', 'outlook', 'outlook_calendar',
      'imap', 'clickup', 'linear', 'ms_teams', 'paperless_ngx', 'nextcloud',
      'google_ads', 'darwinbox'
    ))
    OR
    (integration_type = 'remote_mcp' AND source_type ~ '^[a-z][a-z0-9_-]{1,49}$')
);

-- At most one non-deleted remote MCP source may claim an app slug. Native/native
-- duplicates remain legal for providers that support multiple source rows.
CREATE UNIQUE INDEX sources_remote_mcp_source_type_uniq
    ON sources (source_type)
    WHERE integration_type = 'remote_mcp' AND is_deleted = false;
