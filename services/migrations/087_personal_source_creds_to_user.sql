-- Backfill personal source credentials to per-user rows.
--
-- Migration 086 left scope='user' (personal) sources with credentials in the
-- "org row" shape (user_id IS NULL). Credential resolution is simpler if we
-- treat the rule strictly: user_id Some -> per-user row required;
-- user_id None -> org-wide row. That requires personal-source creds to be
-- per-user rows owned by the source's creator.

UPDATE service_credentials sc
SET user_id = s.created_by
FROM sources s
WHERE sc.source_id = s.id
  AND s.scope = 'user'
  AND sc.user_id IS NULL;
