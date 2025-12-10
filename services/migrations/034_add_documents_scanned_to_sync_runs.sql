-- Add documents_scanned column to sync_runs table
ALTER TABLE sync_runs ADD COLUMN IF NOT EXISTS documents_scanned INTEGER DEFAULT 0;

-- Create trigger function to notify on sync_run updates
CREATE OR REPLACE FUNCTION notify_sync_run_update()
RETURNS TRIGGER AS $$
BEGIN
    PERFORM pg_notify('sync_run_update', '');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Create trigger to automatically notify on any sync_runs update
DROP TRIGGER IF EXISTS sync_run_update_trigger ON sync_runs;
CREATE TRIGGER sync_run_update_trigger
    AFTER UPDATE ON sync_runs
    FOR EACH ROW
    EXECUTE FUNCTION notify_sync_run_update();
