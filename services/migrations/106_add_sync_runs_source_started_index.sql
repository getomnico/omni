-- B-tree index on (source_id, started_at DESC) so DISTINCT ON / ROW_NUMBER
-- window-partition queries can walk the index in order without sorting.  Makes
-- "latest sync run per source" lookups O(1) per source instead of a full sort
-- of the entire sync_runs table.

CREATE INDEX IF NOT EXISTS idx_sync_runs_source_started
    ON sync_runs (source_id, started_at DESC);
