-- Document authorization is enforced by PostgreSQL rather than by individual query paths.
-- User-facing services log in as omni_user and the background services as omni_system; each
-- role is a real login with a password provisioned by run-migrations.sh. The login/migration
-- role remains the table owner so migrations can continue to run, but ordinary requests run
-- as omni_user and therefore cannot bypass RLS as the owner.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'omni_user') THEN
        CREATE ROLE omni_user LOGIN NOSUPERUSER NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'omni_system') THEN
        CREATE ROLE omni_system LOGIN NOSUPERUSER NOBYPASSRLS;
    END IF;
END
$$;

ALTER ROLE omni_user LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
ALTER ROLE omni_system LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;

DO $$
BEGIN
    EXECUTE format(
        'GRANT omni_user, omni_system TO %I WITH INHERIT FALSE, SET TRUE',
        current_user
    );
END
$$;

GRANT USAGE ON SCHEMA public TO omni_user, omni_system;
GRANT SELECT ON users, groups, group_memberships, sources, embedding_providers TO omni_user;
GRANT SELECT ON documents, embeddings, content_blobs TO omni_user;
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO omni_system;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO omni_system;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO omni_system;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE ON SEQUENCES TO omni_system;

CREATE OR REPLACE FUNCTION public.omni_document_viewer_email()
RETURNS text
LANGUAGE sql
STABLE
PARALLEL SAFE
AS $$
    SELECT NULLIF(current_setting('omni.document_user_email', true), '')
$$;

CREATE OR REPLACE FUNCTION public.omni_document_access_scope()
RETURNS text
LANGUAGE sql
STABLE
PARALLEL SAFE
AS $$
    SELECT COALESCE(NULLIF(current_setting('omni.document_access_scope', true), ''), 'none')
$$;

CREATE OR REPLACE FUNCTION public.omni_can_read_document(document_permissions jsonb)
RETURNS boolean
LANGUAGE sql
STABLE
PARALLEL SAFE
AS $$
    SELECT CASE omni_document_access_scope()
        WHEN 'public' THEN document_permissions @> '{"public": true}'::jsonb
        WHEN 'user' THEN
            document_permissions @> '{"public": true}'::jsonb
            OR document_permissions->'users' ? omni_document_viewer_email()
            OR (
                split_part(omni_document_viewer_email(), '@', 2) <> ''
                AND document_permissions->'groups' ? split_part(omni_document_viewer_email(), '@', 2)
            )
            OR EXISTS (
                SELECT 1
                FROM group_memberships gm
                JOIN groups g ON g.id = gm.group_id
                WHERE lower(gm.member_email) = lower(omni_document_viewer_email())
                  AND document_permissions->'groups' ? g.email
            )
        ELSE false
    END
$$;

REVOKE ALL ON FUNCTION public.omni_can_read_document(jsonb) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.omni_can_read_document(jsonb) TO omni_user;

ALTER TABLE documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE documents FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS documents_user_select ON documents;
DROP POLICY IF EXISTS documents_system_all ON documents;
CREATE POLICY documents_user_select ON documents
    FOR SELECT TO omni_user
    USING (
        current_user = 'omni_user'
        AND omni_can_read_document(permissions)
    );
CREATE POLICY documents_system_all ON documents
    FOR ALL TO omni_system
    USING (current_user = 'omni_system')
    WITH CHECK (current_user = 'omni_system');

ALTER TABLE embeddings ENABLE ROW LEVEL SECURITY;
ALTER TABLE embeddings FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS embeddings_user_select ON embeddings;
DROP POLICY IF EXISTS embeddings_system_all ON embeddings;
CREATE POLICY embeddings_user_select ON embeddings
    FOR SELECT TO omni_user
    USING (
        current_user = 'omni_user'
        AND EXISTS (SELECT 1 FROM documents d WHERE d.id = embeddings.document_id)
    );
CREATE POLICY embeddings_system_all ON embeddings
    FOR ALL TO omni_system
    USING (current_user = 'omni_system')
    WITH CHECK (current_user = 'omni_system');

ALTER TABLE content_blobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE content_blobs FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS content_blobs_user_select ON content_blobs;
DROP POLICY IF EXISTS content_blobs_system_all ON content_blobs;
CREATE POLICY content_blobs_user_select ON content_blobs
    FOR SELECT TO omni_user
    USING (
        current_user = 'omni_user'
        AND EXISTS (SELECT 1 FROM documents d WHERE d.content_id = content_blobs.id)
    );
CREATE POLICY content_blobs_system_all ON content_blobs
    FOR ALL TO omni_system
    USING (current_user = 'omni_system')
    WITH CHECK (current_user = 'omni_system');

-- Admin-scoped API keys exposed the entire corpus. Revoke them rather than silently
-- changing their meaning, then constrain all new keys to the documented safe scopes.
UPDATE api_keys
SET is_active = false, scope = 'user', updated_at = NOW()
WHERE scope = 'admin';
ALTER TABLE api_keys DROP CONSTRAINT IF EXISTS api_keys_scope_check;
ALTER TABLE api_keys ADD CONSTRAINT api_keys_scope_check CHECK (scope IN ('public', 'user'));
COMMENT ON COLUMN api_keys.scope IS
    'Permission scope: public = only public documents, user = inherits creating user permissions';
