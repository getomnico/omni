-- Allow 'realtime' as a valid sync_type for long-lived watcher syncs
-- (e.g. filesystem connector under SyncMode::Realtime).
ALTER TABLE sync_runs DROP CONSTRAINT sync_runs_sync_type_check;
ALTER TABLE sync_runs ADD CONSTRAINT sync_runs_sync_type_check
    CHECK (sync_type IN ('full', 'incremental', 'realtime'));
