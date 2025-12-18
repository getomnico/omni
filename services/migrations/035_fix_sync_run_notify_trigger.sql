-- Fix sync_run_update trigger to also fire on INSERT (not just UPDATE)
-- This ensures notifications are sent when new sync runs are created

DROP TRIGGER IF EXISTS sync_run_update_trigger ON sync_runs;
CREATE TRIGGER sync_run_update_trigger
    AFTER INSERT OR UPDATE ON sync_runs
    FOR EACH ROW
    EXECUTE FUNCTION notify_sync_run_update();
