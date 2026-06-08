-- Add explicit sync checkpoint storage separate from connector metadata.
-- `sync_runs.checkpoint` stores in-progress/resume checkpoints for a single run.
-- `sources.checkpoint` stores the checkpoint from the latest successfully completed sync.
-- Existing connector_state is left unchanged. Checkpoints start empty after
-- upgrade, so sources need a full sync to establish the first checkpoint.

ALTER TABLE sync_runs
ADD COLUMN IF NOT EXISTS checkpoint JSONB;

ALTER TABLE sources
ADD COLUMN IF NOT EXISTS checkpoint JSONB;
