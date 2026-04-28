-- Per-user OAuth credentials for org-wide sources, plus encryption of user_oauth_credentials.
--
-- Adds:
--   sources.scope                       -- 'org' | 'user'
--   service_credentials.user_id         -- NULL = org-wide cred; NOT NULL = per-user cred for an org-wide source
--   user_oauth_credentials.access_token -- TEXT -> JSONB ({encrypted_data, version}); table is truncated, users re-auth on next sign-in
--   user_oauth_credentials.refresh_token -- TEXT -> JSONB ({encrypted_data, version})

-- ---------------------------------------------------------------------------
-- sources.scope
-- ---------------------------------------------------------------------------

ALTER TABLE sources ADD COLUMN scope TEXT NOT NULL DEFAULT 'user';
ALTER TABLE sources ADD CONSTRAINT sources_scope_check CHECK (scope IN ('org', 'user'));

-- Backfill: a source is org-scoped iff it has a service-account-style credential.
-- OAuth-typed credentials stay user-scoped (matches the existing user-OAuth flow's intent).
UPDATE sources SET scope = 'org'
WHERE id IN (
    SELECT s.id
    FROM sources s
    JOIN service_credentials sc ON sc.source_id = s.id
    WHERE sc.auth_type IN ('jwt', 'api_key', 'basic_auth', 'bearer_token', 'bot_token')
);

-- Uniqueness of org sources per source_type is enforced at the application
-- layer (see web/src/routes/api/sources/+server.ts) — some source_types
-- legitimately allow multiple org-wide sources (e.g., 'web' for indexing
-- multiple sites).

CREATE INDEX idx_sources_scope ON sources (scope);

COMMENT ON COLUMN sources.scope IS
    'org = admin-set-up source shared by all users; user = personal source owned by created_by';

-- ---------------------------------------------------------------------------
-- service_credentials.user_id
-- ---------------------------------------------------------------------------

ALTER TABLE service_credentials
    ADD COLUMN user_id CHAR(26) REFERENCES users(id) ON DELETE CASCADE;

-- Replace the (source_id, provider) unique with two partial uniques on user_id.
-- One org-wide row per source (user_id IS NULL), and at most one per-user row per (source, user).
DROP INDEX IF EXISTS idx_service_credentials_source_provider;

CREATE UNIQUE INDEX service_credentials_source_user_uniq
    ON service_credentials (source_id, user_id)
    WHERE user_id IS NOT NULL;

CREATE UNIQUE INDEX service_credentials_source_org_uniq
    ON service_credentials (source_id)
    WHERE user_id IS NULL;

-- Non-unique index for "list all credentials owned by a given user" lookups.
-- Uniqueness is on (source_id, user_id) above — a user can hold multiple
-- credentials, but at most one per source.
CREATE INDEX service_credentials_user_id_idx
    ON service_credentials (user_id) WHERE user_id IS NOT NULL;

COMMENT ON COLUMN service_credentials.user_id IS
    'NULL = org-wide credential (used for sync and reads); NOT NULL = per-user credential for an org-wide source (used for write tools by that user)';

-- ---------------------------------------------------------------------------
-- user_oauth_credentials: encrypt tokens at rest (breaking change; users re-auth on next sign-in)
-- ---------------------------------------------------------------------------

TRUNCATE TABLE user_oauth_credentials;

ALTER TABLE user_oauth_credentials
    ALTER COLUMN access_token  TYPE JSONB USING NULL,
    ALTER COLUMN refresh_token TYPE JSONB USING NULL;

ALTER TABLE user_oauth_credentials
    ADD CONSTRAINT user_oauth_credentials_access_token_encrypted
        CHECK (access_token  IS NULL OR (access_token  ? 'encrypted_data' AND access_token  ? 'version')),
    ADD CONSTRAINT user_oauth_credentials_refresh_token_encrypted
        CHECK (refresh_token IS NULL OR (refresh_token ? 'encrypted_data' AND refresh_token ? 'version'));

COMMENT ON COLUMN user_oauth_credentials.access_token IS
    'Encrypted access token. JSONB shape: {encrypted_data: {...}, version: 1}';
COMMENT ON COLUMN user_oauth_credentials.refresh_token IS
    'Encrypted refresh token. JSONB shape: {encrypted_data: {...}, version: 1}';
