-- Harden agent_runs into a durable DB-backed queue for background agents.
-- agent_runs is the queue; agent_run_logs is the durable conversation/action WAL.

ALTER TABLE agent_runs
    ADD COLUMN IF NOT EXISTS trigger_type TEXT NOT NULL DEFAULT 'manual',
    ADD COLUMN IF NOT EXISTS claim_token CHAR(26),
    ADD COLUMN IF NOT EXISTS lease_expires_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS heartbeat_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS attempt_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS max_attempts INTEGER NOT NULL DEFAULT 3;

ALTER TABLE agent_runs
    DROP COLUMN IF EXISTS execution_log;

ALTER TABLE agent_runs
    ADD CONSTRAINT agent_runs_trigger_type_check
        CHECK (trigger_type IN ('manual', 'scheduled')),
    ADD CONSTRAINT agent_runs_attempt_count_check
        CHECK (attempt_count >= 0),
    ADD CONSTRAINT agent_runs_max_attempts_check
        CHECK (max_attempts >= 1),
    ADD CONSTRAINT agent_runs_running_lease_check
        CHECK (
            status <> 'running'
            OR (claim_token IS NOT NULL AND lease_expires_at IS NOT NULL AND heartbeat_at IS NOT NULL)
        );

CREATE TABLE IF NOT EXISTS agent_run_logs (
    id CHAR(26) PRIMARY KEY,
    run_id CHAR(26) NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    message_seq_num INTEGER NOT NULL,
    message JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT agent_run_logs_seq_num_check CHECK (message_seq_num >= 0),
    CONSTRAINT agent_run_logs_run_seq_unique UNIQUE (run_id, message_seq_num)
);

CREATE INDEX IF NOT EXISTS idx_agent_runs_pending_created
    ON agent_runs(created_at)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_agent_runs_active_by_agent
    ON agent_runs(agent_id, status, lease_expires_at)
    WHERE status IN ('pending', 'running');

CREATE INDEX IF NOT EXISTS idx_agent_runs_stale_running
    ON agent_runs(lease_expires_at)
    WHERE status = 'running';

CREATE INDEX IF NOT EXISTS idx_agent_run_logs_run_seq
    ON agent_run_logs(run_id, message_seq_num);
