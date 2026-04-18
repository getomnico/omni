-- ============================================================
-- RLS: Row Level Security for multi-user isolation
-- ============================================================
-- All user-scoped tables are now protected by RLS policies.
-- Application code sets `app.current_user_id` session variable
-- to the authenticated user's ID before executing queries.
-- Policies reference this via `current_setting('app.current_user_id')`.
-- ============================================================

-- ============================================================
-- Helper functions
-- ============================================================

-- Resolve the authenticated user's email from their ID.
-- Returns NULL if user_id is not set or user not found.
CREATE OR REPLACE FUNCTION public._viewer_email()
RETURNS text
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = public
AS $$
  SELECT email FROM users WHERE id = current_setting('app.current_user_id')::CHAR(26)
$$;

-- Check if the current session has admin privileges.
-- Application sets `app.is_admin = 'true'` for admin operations.
CREATE OR REPLACE FUNCTION public._is_admin()
RETURNS boolean
LANGUAGE sql
STABLE
AS $$
  SELECT current_setting('app.is_admin', true) = 'true'
$$;

-- ============================================================
-- documents table — permission-based access
-- ============================================================

-- Enable RLS (NO FORCE for phased rollout)
ALTER TABLE documents ENABLE ROW LEVEL SECURITY;

-- User can see documents they have permission to access.
-- Checks: public, direct user email, domain-wide, group membership.
CREATE POLICY documents_user_select ON documents
  FOR SELECT
  USING (
    permissions @> '{"public": true}'::jsonb
    OR permissions->'users' ? _viewer_email()
    OR (
      split_part(_viewer_email(), '@', 2) IS NOT NULL
      AND permissions->'groups' ? split_part(_viewer_email(), '@', 2)
    )
    OR EXISTS (
      SELECT 1
      FROM group_memberships gm
      JOIN groups g ON g.id = gm.group_id
      WHERE lower(gm.member_email) = lower(_viewer_email())
        AND g.email::text = ANY(
          (SELECT jsonb_array_elements_text(permissions->'groups'))
        )
    )
  );

-- Indexer (runs as privileged role) can insert/update/delete.
CREATE POLICY documents_indexer_insert ON documents
  FOR INSERT WITH CHECK (_is_admin());

CREATE POLICY documents_indexer_update ON documents
  FOR UPDATE USING (_is_admin()) WITH CHECK (_is_admin());

CREATE POLICY documents_indexer_delete ON documents
  FOR DELETE USING (_is_admin());

-- Admin bypass: admins can see all documents.
CREATE POLICY documents_admin_select ON documents
  FOR SELECT
  USING (_is_admin());

-- ============================================================
-- embeddings table — follows document permissions
-- ============================================================

ALTER TABLE embeddings ENABLE ROW LEVEL SECURITY;

CREATE POLICY embeddings_user_select ON embeddings
  FOR SELECT
  USING (
    EXISTS (
      SELECT 1 FROM documents d
      WHERE d.id = embeddings.document_id
        AND (
          d.permissions @> '{"public": true}'::jsonb
          OR d.permissions->'users' ? _viewer_email()
          OR (
            split_part(_viewer_email(), '@', 2) IS NOT NULL
            AND d.permissions->'groups' ? split_part(_viewer_email(), '@', 2)
          )
          OR EXISTS (
            SELECT 1
            FROM group_memberships gm
            JOIN groups g ON g.id = gm.group_id
            WHERE lower(gm.member_email) = lower(_viewer_email())
              AND g.email::text = ANY(
                (SELECT jsonb_array_elements_text(d.permissions->'groups'))
              )
          )
        )
    )
    OR _is_admin()
  );

CREATE POLICY embeddings_indexer_insert ON embeddings
  FOR INSERT WITH CHECK (_is_admin());

CREATE POLICY embeddings_indexer_delete ON embeddings
  FOR DELETE USING (_is_admin());

-- ============================================================
-- chats table — user-owned
-- ============================================================

ALTER TABLE chats ENABLE ROW LEVEL SECURITY;

CREATE POLICY chats_user_select ON chats
  FOR SELECT
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY chats_user_insert ON chats
  FOR INSERT
  WITH CHECK (user_id = current_setting('app.current_user_id')::CHAR(26));

CREATE POLICY chats_user_update ON chats
  FOR UPDATE
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY chats_user_delete ON chats
  FOR DELETE
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

-- ============================================================
-- chat_messages table — accessible via chat ownership
-- ============================================================

ALTER TABLE chat_messages ENABLE ROW LEVEL SECURITY;

CREATE POLICY chat_messages_user_select ON chat_messages
  FOR SELECT
  USING (
    EXISTS (
      SELECT 1 FROM chats WHERE chats.id = chat_messages.chat_id
        AND (chats.user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin())
    )
  );

CREATE POLICY chat_messages_user_insert ON chat_messages
  FOR INSERT
  WITH CHECK (
    EXISTS (
      SELECT 1 FROM chats WHERE chats.id = chat_messages.chat_id
        AND chats.user_id = current_setting('app.current_user_id')::CHAR(26)
    )
  );

-- ============================================================
-- agents table — user-owned
-- ============================================================

ALTER TABLE agents ENABLE ROW LEVEL SECURITY;

CREATE POLICY agents_user_select ON agents
  FOR SELECT
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY agents_user_insert ON agents
  FOR INSERT
  WITH CHECK (user_id = current_setting('app.current_user_id')::CHAR(26));

CREATE POLICY agents_user_update ON agents
  FOR UPDATE
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY agents_user_delete ON agents
  FOR DELETE
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

-- ============================================================
-- agent_runs table — accessible via agent ownership
-- ============================================================

ALTER TABLE agent_runs ENABLE ROW LEVEL SECURITY;

CREATE POLICY agent_runs_user_select ON agent_runs
  FOR SELECT
  USING (
    EXISTS (
      SELECT 1 FROM agents WHERE agents.id = agent_runs.agent_id
        AND (agents.user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin())
    )
  );

-- ============================================================
-- api_keys table — user-owned
-- ============================================================

ALTER TABLE api_keys ENABLE ROW LEVEL SECURITY;

CREATE POLICY api_keys_user_select ON api_keys
  FOR SELECT
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY api_keys_user_insert ON api_keys
  FOR INSERT
  WITH CHECK (user_id = current_setting('app.current_user_id')::CHAR(26));

CREATE POLICY api_keys_user_update ON api_keys
  FOR UPDATE
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY api_keys_user_delete ON api_keys
  FOR DELETE
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

-- ============================================================
-- uploads table — user-owned
-- ============================================================

ALTER TABLE uploads ENABLE ROW LEVEL SECURITY;

CREATE POLICY uploads_user_select ON uploads
  FOR SELECT
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY uploads_user_insert ON uploads
  FOR INSERT
  WITH CHECK (user_id = current_setting('app.current_user_id')::CHAR(26));

CREATE POLICY uploads_user_delete ON uploads
  FOR DELETE
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

-- ============================================================
-- response_feedback table — user-owned
-- ============================================================

ALTER TABLE response_feedback ENABLE ROW LEVEL SECURITY;

CREATE POLICY response_feedback_user_select ON response_feedback
  FOR SELECT
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY response_feedback_user_insert ON response_feedback
  FOR INSERT
  WITH CHECK (user_id = current_setting('app.current_user_id')::CHAR(26));

CREATE POLICY response_feedback_user_delete ON response_feedback
  FOR DELETE
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

-- ============================================================
-- tool_approvals table — user-owned
-- ============================================================

ALTER TABLE tool_approvals ENABLE ROW LEVEL SECURITY;

CREATE POLICY tool_approvals_user_select ON tool_approvals
  FOR SELECT
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY tool_approvals_user_insert ON tool_approvals
  FOR INSERT
  WITH CHECK (user_id = current_setting('app.current_user_id')::CHAR(26));

CREATE POLICY tool_approvals_user_update ON tool_approvals
  FOR UPDATE
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY tool_approvals_user_delete ON tool_approvals
  FOR DELETE
  USING (user_id = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

-- ============================================================
-- magic_links table — user-owned (read by userId, write by system)
-- ============================================================

ALTER TABLE magic_links ENABLE ROW LEVEL SECURITY;

CREATE POLICY magic_links_user_select ON magic_links
  FOR SELECT
  USING (
    user_id IS NULL  -- unused/expiring magic links are system-visible
    OR user_id = current_setting('app.current_user_id')::CHAR(26)
    OR _is_admin()
  );

CREATE POLICY magic_links_system_insert ON magic_links
  FOR INSERT
  WITH CHECK (user_id IS NOT NULL OR _is_admin());

CREATE POLICY magic_links_system_update ON magic_links
  FOR UPDATE
  USING (_is_admin());

-- ============================================================
-- sources table — created_by isolation
-- ============================================================

ALTER TABLE sources ENABLE ROW LEVEL SECURITY;

CREATE POLICY sources_user_select ON sources
  FOR SELECT
  USING (created_by = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY sources_user_insert ON sources
  FOR INSERT
  WITH CHECK (created_by = current_setting('app.current_user_id')::CHAR(26));

CREATE POLICY sources_user_update ON sources
  FOR UPDATE
  USING (created_by = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY sources_user_delete ON sources
  FOR DELETE
  USING (created_by = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

-- ============================================================
-- approved_domains table — approved_by isolation
-- ============================================================

ALTER TABLE approved_domains ENABLE ROW LEVEL SECURITY;

CREATE POLICY approved_domains_user_select ON approved_domains
  FOR SELECT
  USING (approved_by = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

CREATE POLICY approved_domains_user_insert ON approved_domains
  FOR INSERT
  WITH CHECK (approved_by = current_setting('app.current_user_id')::CHAR(26));

CREATE POLICY approved_domains_user_delete ON approved_domains
  FOR DELETE
  USING (approved_by = current_setting('app.current_user_id')::CHAR(26) OR _is_admin());

-- ============================================================
-- people table — group membership based access
-- ============================================================

-- Users can see people records for members of groups they belong to
-- (for search autocomplete / @mentions). Also visible if the person
-- IS the current user.
ALTER TABLE people ENABLE ROW LEVEL SECURITY;

CREATE POLICY people_user_select ON people
  FOR SELECT
  USING (
    email = _viewer_email()
    OR _is_admin()
    OR EXISTS (
      SELECT 1 FROM group_memberships gm
      WHERE lower(gm.member_email) = lower(_viewer_email())
        AND gm.group_id IN (
          SELECT id FROM groups g
          WHERE lower(g.email) = lower(people.email)
        )
    )
  );

-- Indexer/admin can insert/update/delete people records.
CREATE POLICY people_indexer_insert ON people
  FOR INSERT WITH CHECK (_is_admin());

CREATE POLICY people_indexer_update ON people
  FOR UPDATE USING (_is_admin()) WITH CHECK (_is_admin());

CREATE POLICY people_indexer_delete ON people
  FOR DELETE USING (_is_admin());

-- ============================================================
-- groups table — visible to members
-- ============================================================

ALTER TABLE groups ENABLE ROW LEVEL SECURITY;

CREATE POLICY groups_user_select ON groups
  FOR SELECT
  USING (
    _is_admin()
    OR EXISTS (
      SELECT 1 FROM group_memberships gm
      WHERE gm.group_id = groups.id
        AND lower(gm.member_email) = lower(_viewer_email())
    )
  );

-- Indexer/admin can insert/update/delete groups.
CREATE POLICY groups_indexer_insert ON groups
  FOR INSERT WITH CHECK (_is_admin());

CREATE POLICY groups_indexer_update ON groups
  FOR UPDATE USING (_is_admin()) WITH CHECK (_is_admin());

CREATE POLICY groups_indexer_delete ON groups
  FOR DELETE USING (_is_admin());

-- ============================================================
-- group_memberships table — visible to members
-- ============================================================

ALTER TABLE group_memberships ENABLE ROW LEVEL SECURITY;

CREATE POLICY group_memberships_user_select ON group_memberships
  FOR SELECT
  USING (
    _is_admin()
    OR EXISTS (
      SELECT 1 FROM group_memberships gm2
      WHERE gm2.group_id = group_memberships.group_id
        AND lower(gm2.member_email) = lower(_viewer_email())
    )
  );

-- Indexer/admin can insert/update/delete group memberships.
CREATE POLICY group_memberships_indexer_insert ON group_memberships
  FOR INSERT WITH CHECK (_is_admin());

CREATE POLICY group_memberships_indexer_update ON group_memberships
  FOR UPDATE USING (_is_admin()) WITH CHECK (_is_admin());

CREATE POLICY group_memberships_indexer_delete ON group_memberships
  FOR DELETE USING (_is_admin());
