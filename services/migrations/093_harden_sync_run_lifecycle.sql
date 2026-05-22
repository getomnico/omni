-- Enforce one running sync per source per slot class.
-- Realtime syncs occupy their own slot; full/incremental share the scheduled slot.

WITH duplicate_running AS (
    SELECT
        id,
        ROW_NUMBER() OVER (
            PARTITION BY source_id,
                         CASE WHEN sync_type = 'realtime' THEN 'realtime' ELSE 'scheduled' END
            ORDER BY started_at DESC NULLS LAST, created_at DESC, id DESC
        ) AS rn
    FROM sync_runs
    WHERE status = 'running'
)
UPDATE sync_runs sr
SET status = 'failed',
    completed_at = COALESCE(sr.completed_at, NOW()),
    error_message = COALESCE(sr.error_message, 'Marked failed before sync slot uniqueness migration'),
    updated_at = NOW()
FROM duplicate_running d
WHERE sr.id = d.id
  AND d.rn > 1;

CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_runs_one_running_per_source_slot
ON sync_runs (
    source_id,
    (CASE WHEN sync_type = 'realtime' THEN 'realtime' ELSE 'scheduled' END)
)
WHERE status = 'running';
