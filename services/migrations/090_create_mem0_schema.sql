-- Isolate mem0's auto-created tables in their own schema so they don't
-- co-mingle with the app's migration-managed tables in `public`.
-- Same DB role as the rest of the app (no separate user) -- this is a
-- namespacing change only, not a trust-boundary change.
CREATE SCHEMA IF NOT EXISTS mem0;
