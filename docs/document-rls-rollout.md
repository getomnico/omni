# Document RLS deployment and rollback

Document row-level security is enforced only when runtime services connect with the non-owner credentials and enter an explicit document role.

## Required secrets

- `DATABASE_USERNAME` / `DATABASE_PASSWORD`: migration and table-owner credentials. Only the migrator should receive these.
- `DATABASE_RUNTIME_USERNAME` / `DATABASE_RUNTIME_PASSWORD`: non-superuser runtime login used by web, searcher, and AI. It can only assume the `omni_documents_user` role.
- `DATABASE_SYSTEM_USERNAME` / `DATABASE_SYSTEM_PASSWORD`: privileged runtime login used by indexer, connector-manager, and by searcher/AI for system-scoped storage reads. It can only assume the `omni_documents_system` role; it has no direct table grants.
- `OMNI_INTERNAL_SERVICE_TOKEN`: random service-to-service secret. When configured, the searcher requires it on every identity-bearing request so a direct caller cannot forge a user identity.

The migrator creates the two NOLOGIN document roles, provisions the two runtime logins, grants only role-switch capability, and revokes direct access to `documents`, `embeddings`, and `content_blobs`. User-facing services must never receive the system login, and the user login must never hold membership in `omni_documents_system`.

## Upgrade order

1. Generate and distribute the runtime database password and internal service token.
2. Deploy the migrator first. Migration 112 enables and forces RLS, deactivates existing `admin` API keys, and limits new keys to `public` or `user` scope.
3. Confirm the runtime role is `NOSUPERUSER NOBYPASSRLS`, is not the `documents` owner, and has no direct table privilege.
4. Deploy indexer and connector-manager, then AI/searcher, then web.
5. Canary with two users that have disjoint private documents. Verify search, typeahead, direct reads, facets/counts, and AI document tools in both directions.
6. Monitor errors for missing document context or failed `SET ROLE` operations before broad rollout.

Do not deploy runtime services with the migration/table-owner credentials. `FORCE ROW LEVEL SECURITY` is defense in depth but does not constrain superusers or `BYPASSRLS` roles.

## Validation queries

```sql
SELECT relrowsecurity, relforcerowsecurity
FROM pg_class WHERE oid = 'documents'::regclass;

SELECT rolname, rolsuper, rolbypassrls
FROM pg_roles
WHERE rolname IN ('omni_documents_user', 'omni_documents_system', 'omni_runtime');

SELECT grantee, privilege_type
FROM information_schema.role_table_grants
WHERE table_name = 'documents';
```

Use `EXPLAIN (ANALYZE, BUFFERS)` under `SET LOCAL ROLE omni_documents_user` and transaction-local context for representative BM25, vector-join, typeahead candidate, direct-ID, facet, and count queries. Compare latency, buffers, and candidate counts with the pre-RLS baseline.

## Known follow-ups

- Web keeps one transaction open for the full authenticated request (`hooks.server.ts`) to scope `locals.db`. This is correct but can hold a connection for slow SSR/stream requests; a route-scoped document transaction helper should eventually replace it.
- AI user-path content reads run through the system storage pool after the document permission check has already passed; the RLS user scope still protects direct and search-path access to blobs. A constrained security-definer blob accessor would tighten this further.
- Managed cloud deployments must add the two runtime login credentials and the internal service token to their secret stores; compose and the migrator already support them.

## Rollback

Prefer rolling application code forward. If rollback is necessary:

1. Keep migration 112 and RLS enabled.
2. Roll back only to an application version that understands runtime credentials and document roles.
3. Do not restore `admin` API keys and do not disable or bypass RLS for normal traffic.
4. For emergency operational repair, use the migration credential during a bounded maintenance window; never expose it to a user-facing service.

A code rollback to a version that does not establish document context will fail closed on document reads rather than leak rows.
