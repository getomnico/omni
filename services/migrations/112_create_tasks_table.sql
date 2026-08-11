-- Generic PostgreSQL task queue.
--
-- One table backs every kind of queued work (connector events, document
-- embeddings, agent runs, ...). The lifecycle state machine lives in the
-- task_* functions below; the shared Rust (`shared::task_queue`) and Python
-- (`db.task_queue`) facades only translate typed arguments to those
-- functions, so both languages share exactly one implementation.
--
-- Workload-specific payloads live in `payload` and are interpreted by
-- (task_type, payload_version). Delivery is at-least-once: a task can be
-- claimed again after its lease expires, so consumers must be idempotent
-- around database writes and external effects.

CREATE TABLE tasks (
    id                  TEXT PRIMARY KEY,
    task_type           TEXT NOT NULL,
    payload             JSONB NOT NULL,
    payload_version     INTEGER NOT NULL DEFAULT 1,
    status              TEXT NOT NULL DEFAULT 'pending',
    priority            INTEGER NOT NULL DEFAULT 0,
    available_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    weight              BIGINT NOT NULL DEFAULT 1,
    concurrency_key     TEXT,
    attempt_count       INTEGER NOT NULL DEFAULT 0,
    max_attempts        INTEGER NOT NULL DEFAULT 3,
    last_error          TEXT,
    claim_token         TEXT,
    claimed_by          TEXT,
    lease_expires_at    TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_started_at     TIMESTAMPTZ,
    completed_at        TIMESTAMPTZ,

    CONSTRAINT tasks_id_check
        CHECK (char_length(id) = 26),
    CONSTRAINT tasks_task_type_check
        CHECK (btrim(task_type) <> ''),
    CONSTRAINT tasks_payload_check
        CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT tasks_payload_version_check
        CHECK (payload_version >= 1),
    CONSTRAINT tasks_status_check
        CHECK (status IN ('pending', 'running', 'completed', 'dead_letter')),
    CONSTRAINT tasks_weight_check
        CHECK (weight >= 0),
    CONSTRAINT tasks_attempt_count_check
        CHECK (attempt_count >= 0),
    CONSTRAINT tasks_max_attempts_check
        CHECK (max_attempts >= 1),
    CONSTRAINT tasks_attempt_budget_check
        CHECK (attempt_count <= max_attempts),
    CONSTRAINT tasks_claim_token_check
        CHECK (claim_token IS NULL OR char_length(claim_token) = 26),
    CONSTRAINT tasks_running_lease_check
        CHECK (
            (status = 'running'
                AND claim_token IS NOT NULL
                AND claimed_by IS NOT NULL
                AND lease_expires_at IS NOT NULL)
            OR
            (status <> 'running'
                AND claim_token IS NULL
                AND claimed_by IS NULL
                AND lease_expires_at IS NULL)
        ),
    CONSTRAINT tasks_terminal_completed_check
        CHECK (
            (status IN ('completed', 'dead_letter') AND completed_at IS NOT NULL)
            OR
            (status IN ('pending', 'running') AND completed_at IS NULL)
        )
);

-- Fast eligibility scan for the common claim path (by task_type).
CREATE INDEX idx_tasks_pending_claim
    ON tasks (task_type, priority DESC, available_at, id)
    WHERE status = 'pending';

-- Stale-lease recovery scans running tasks whose lease has expired.
CREATE INDEX idx_tasks_running_lease
    ON tasks (lease_expires_at)
    WHERE status = 'running';

-- The oldest-unresolved lookup used to serialize tasks that share a
-- concurrency_key.
CREATE INDEX idx_tasks_unresolved_concurrency
    ON tasks (task_type, concurrency_key, id)
    WHERE status IN ('pending', 'running') AND concurrency_key IS NOT NULL;

-- Hard safeguard: a concurrency_key can never have two running tasks.
-- The claim function already admits at most the oldest unresolved task per
-- key; this index turns any enforcement gap into a loud constraint failure.
CREATE UNIQUE INDEX idx_tasks_running_concurrency_key
    ON tasks (task_type, concurrency_key)
    WHERE status = 'running' AND concurrency_key IS NOT NULL;

COMMENT ON TABLE tasks IS
    'Generic at-least-once task queue. Workload payloads are interpreted by task_type and payload_version.';
COMMENT ON COLUMN tasks.id IS
    'Caller-generated ULID. Producers retrying idempotently reuse the same id; task_enqueue_bulk ignores duplicate ids.';
COMMENT ON COLUMN tasks.task_type IS
    'Opaque consumer category. Planned values: connector_event, document_embedding, agent_run.';
COMMENT ON COLUMN tasks.payload IS
    'Versioned workload input. Connector payloads include type and sync_run_id; embedding payloads include document_id; agent payloads include run_id and agent_id.';
COMMENT ON COLUMN tasks.payload_version IS
    'Schema version of payload within task_type; starts at 1.';
COMMENT ON COLUMN tasks.status IS
    'pending (eligible once available_at passes), running (leased by a worker), completed (successful terminal work), dead_letter (exhausted or non-retryable terminal work).';
COMMENT ON COLUMN tasks.priority IS
    'Higher values claim first. Initial workloads use 0 to preserve FIFO.';
COMMENT ON COLUMN tasks.available_at IS
    'Earliest claim time; also the scheduled retry time after a failure.';
COMMENT ON COLUMN tasks.weight IS
    'Non-negative claim cost. Connector tasks use payload plus content bytes; other initial task types use 1.';
COMMENT ON COLUMN tasks.concurrency_key IS
    'Optional serialization partition. Tasks of one task_type sharing a key execute one at a time in oldest-task order.';
COMMENT ON COLUMN tasks.attempt_count IS
    'Number of claims, including the initial attempt; incremented atomically when claimed.';
COMMENT ON COLUMN tasks.max_attempts IS
    'Maximum number of claims before a task dead-letters.';
COMMENT ON COLUMN tasks.last_error IS
    'Most recent failure or recovery reason; cleared on completion.';
COMMENT ON COLUMN tasks.claim_token IS
    'Per-claim ULID fencing token required for lease renewal and terminal transitions.';
COMMENT ON COLUMN tasks.claimed_by IS
    'Service/worker identifier holding the lease; diagnostics only.';
COMMENT ON COLUMN tasks.lease_expires_at IS
    'Lease deadline while running; a worker that does not renew before this loses the task.';
COMMENT ON COLUMN tasks.created_at IS
    'Enqueue time.';
COMMENT ON COLUMN tasks.updated_at IS
    'Time of the last state transition.';
COMMENT ON COLUMN tasks.last_started_at IS
    'Time of the most recent claim.';
COMMENT ON COLUMN tasks.completed_at IS
    'Terminal time; set only for completed and dead_letter tasks.';

-- ---------------------------------------------------------------------------
-- Canonical queue functions
-- ---------------------------------------------------------------------------

-- Bulk enqueue. Duplicate ids are ignored so a producer retry that reuses an
-- id is a no-op. Returns only the rows that were actually inserted.
-- available_at is expressed in unix epoch milliseconds.
CREATE OR REPLACE FUNCTION task_enqueue_bulk(
    p_ids TEXT[],
    p_task_types TEXT[],
    p_payloads JSONB[],
    p_payload_versions INTEGER[],
    p_priorities INTEGER[],
    p_available_at_ms BIGINT[],
    p_weights BIGINT[],
    p_concurrency_keys TEXT[],
    p_max_attempts INTEGER[]
) RETURNS SETOF tasks AS $$
BEGIN
    RETURN QUERY
    INSERT INTO tasks (
        id, task_type, payload, payload_version,
        priority, available_at, weight, concurrency_key, max_attempts
    )
    SELECT
        ids.id,
        ids.task_type,
        ids.payload,
        ids.payload_version,
        ids.priority,
        to_timestamp(ids.available_at_ms::double precision / 1000.0),
        ids.weight,
        ids.concurrency_key,
        ids.max_attempts
    FROM UNNEST(
        p_ids,
        p_task_types,
        p_payloads,
        p_payload_versions,
        p_priorities,
        p_available_at_ms,
        p_weights,
        p_concurrency_keys,
        p_max_attempts
    ) AS ids(
        id, task_type, payload, payload_version,
        priority, available_at_ms, weight, concurrency_key, max_attempts
    )
    ON CONFLICT (id) DO NOTHING
    RETURNING *;
END;
$$ LANGUAGE plpgsql;

-- Atomic batch claim. Selects eligible pending tasks (by task_type, or by
-- caller-chosen ids when p_candidate_ids is set), transitions them to
-- running with a shared claim token and lease, increments attempt_count, and
-- returns the claimed rows.
--
-- Ordering: priority DESC, then available_at, then id (ULID) so FIFO is
-- preserved within a priority class. UPDATE ... RETURNING does not preserve
-- this order, so callers that need it must sort by id.
--
-- Weighted batches: p_max_weight caps the cumulative weight of the batch;
-- the first eligible task is always admitted even if it alone exceeds the
-- cap (mirrors the indexer byte budget).
--
-- Concurrency: p_max_concurrency caps running tasks per task_type; the cap
-- is enforced atomically via an advisory lock so concurrent workers cannot
-- over-admit. The concurrency_key rule (oldest unresolved only) applies to
-- every claim, and the unique running index guards the invariant.
CREATE OR REPLACE FUNCTION task_claim_bulk(
    p_task_type TEXT,
    p_candidate_ids TEXT[],
    p_limit INTEGER,
    p_max_weight BIGINT,
    p_max_concurrency INTEGER,
    p_lease_seconds INTEGER,
    p_claim_token TEXT,
    p_claimed_by TEXT
) RETURNS SETOF tasks AS $$
DECLARE
    v_candidate_id TEXT;
    v_now TIMESTAMPTZ;
BEGIN
    IF p_task_type IS NULL OR btrim(p_task_type) = '' THEN
        RAISE EXCEPTION 'task_claim_bulk: task_type must not be empty';
    END IF;
    IF p_claimed_by IS NULL OR btrim(p_claimed_by) = '' THEN
        RAISE EXCEPTION 'task_claim_bulk: claimed_by must not be empty';
    END IF;
    IF p_claim_token IS NULL OR char_length(p_claim_token) <> 26 THEN
        RAISE EXCEPTION 'task_claim_bulk: claim_token must be a 26-char ULID';
    END IF;
    IF p_limit IS NULL OR p_limit < 1 THEN
        RAISE EXCEPTION 'task_claim_bulk: limit must be >= 1';
    END IF;
    IF p_lease_seconds IS NULL OR p_lease_seconds < 1 THEN
        RAISE EXCEPTION 'task_claim_bulk: lease_seconds must be >= 1';
    END IF;
    IF p_max_weight IS NOT NULL AND p_max_weight < 0 THEN
        RAISE EXCEPTION 'task_claim_bulk: max_weight must be >= 0';
    END IF;
    IF p_max_concurrency IS NOT NULL AND p_max_concurrency < 1 THEN
        RAISE EXCEPTION 'task_claim_bulk: max_concurrency must be >= 1';
    END IF;
    IF p_candidate_ids IS NOT NULL THEN
        FOREACH v_candidate_id IN ARRAY p_candidate_ids LOOP
            IF v_candidate_id IS NULL OR char_length(v_candidate_id) <> 26 THEN
                RAISE EXCEPTION 'task_claim_bulk: candidate task id must be a 26-char ULID: %', v_candidate_id;
            END IF;
        END LOOP;
    END IF;

    IF p_max_concurrency IS NOT NULL THEN
        -- Serialize claims per task_type so the running-count check and the
        -- state transition below are atomic across workers.
        PERFORM pg_advisory_xact_lock(hashtext(p_task_type)::bigint);
    END IF;

    -- clock_timestamp() keeps advancing while this statement runs, so a
    -- lease created after a long advisory-lock wait is measured from the
    -- moment the claim actually proceeds.
    v_now := clock_timestamp();

    RETURN QUERY
    WITH candidates AS (
        SELECT c.id, c.priority, c.available_at, c.weight
        FROM tasks c
        WHERE c.status = 'pending'
          AND c.available_at <= v_now
          AND c.attempt_count < c.max_attempts
          AND c.task_type = p_task_type
          AND (p_candidate_ids IS NULL OR c.id = ANY(p_candidate_ids))
          AND (
              c.concurrency_key IS NULL
              OR NOT EXISTS (
                  SELECT 1 FROM tasks older
                  WHERE older.task_type = c.task_type
                    AND older.concurrency_key = c.concurrency_key
                    AND older.status IN ('pending', 'running')
                    AND older.id < c.id
              )
          )
        ORDER BY c.priority DESC, c.available_at, c.id
        LIMIT p_limit
        FOR UPDATE SKIP LOCKED
    ),
    -- MATERIALIZED: without it the planner inlines `ranked` into `batch` and
    -- drops the WHERE filter when the query has two window functions
    -- (observed on PostgreSQL 16/17), returning every candidate regardless of
    -- the weight/concurrency conditions.
    ranked AS MATERIALIZED (
        SELECT id,
               row_number() OVER (
                   ORDER BY priority DESC, available_at, id
               ) AS row_num,
               SUM(weight) OVER (
                   ORDER BY priority DESC, available_at, id
                   ROWS UNBOUNDED PRECEDING
               ) AS running_weight
        FROM candidates
    ),
    batch AS (
        SELECT id
        FROM ranked
        WHERE (
                p_max_weight IS NULL
                OR row_num = 1
                OR running_weight <= p_max_weight
              )
          AND (
                p_max_concurrency IS NULL
                OR row_num <= p_max_concurrency - (
                    SELECT COUNT(*)::INTEGER FROM tasks active
                    WHERE active.task_type = p_task_type
                      AND active.status = 'running'
                      AND active.lease_expires_at > v_now
                )
              )
    )
    UPDATE tasks t
    SET status = 'running',
        claim_token = p_claim_token,
        claimed_by = p_claimed_by,
        lease_expires_at = v_now + make_interval(secs => p_lease_seconds),
        last_started_at = v_now,
        attempt_count = t.attempt_count + 1,
        updated_at = v_now
    FROM batch
    WHERE t.id = batch.id
    RETURNING t.*;
END;
$$ LANGUAGE plpgsql;

-- Renew a running task's lease. Returns false when the task is not running
-- under this token or its lease has already expired (the worker has lost the
-- task and must stop processing it).
CREATE OR REPLACE FUNCTION task_heartbeat(
    p_task_id TEXT,
    p_claim_token TEXT,
    p_lease_seconds INTEGER
) RETURNS BOOLEAN AS $$
DECLARE
    v_updated BIGINT;
    v_now TIMESTAMPTZ;
BEGIN
    IF p_task_id IS NULL OR char_length(p_task_id) <> 26 THEN
        RAISE EXCEPTION 'task_heartbeat: task id must be a 26-char ULID';
    END IF;
    IF p_claim_token IS NULL OR char_length(p_claim_token) <> 26 THEN
        RAISE EXCEPTION 'task_heartbeat: claim_token must be a 26-char ULID';
    END IF;
    IF p_lease_seconds IS NULL OR p_lease_seconds < 1 THEN
        RAISE EXCEPTION 'task_heartbeat: lease_seconds must be >= 1';
    END IF;

    -- Lock the row before taking a timestamp so a concurrent updater cannot
    -- push the lease check past its real wall-clock expiry.
    PERFORM 1 FROM tasks WHERE id = p_task_id FOR UPDATE;
    IF NOT FOUND THEN
        RETURN false;
    END IF;

    v_now := clock_timestamp();

    UPDATE tasks
    SET lease_expires_at = v_now + make_interval(secs => p_lease_seconds),
        updated_at = v_now
    WHERE id = p_task_id
      AND claim_token = p_claim_token
      AND status = 'running'
      AND lease_expires_at > v_now;
    GET DIAGNOSTICS v_updated = ROW_COUNT;
    RETURN v_updated = 1;
END;
$$ LANGUAGE plpgsql;

-- Mark claimed tasks completed. Fenced by the batch claim token and an
-- unexpired lease. Returns the number of tasks completed.
CREATE OR REPLACE FUNCTION task_complete_bulk(
    p_task_ids TEXT[],
    p_claim_token TEXT
) RETURNS BIGINT AS $$
DECLARE
    v_updated BIGINT;
    v_now TIMESTAMPTZ;
BEGIN
    IF p_task_ids IS NULL OR cardinality(p_task_ids) = 0 THEN
        RAISE EXCEPTION 'task_complete_bulk: task_ids must not be empty';
    END IF;
    IF p_claim_token IS NULL OR char_length(p_claim_token) <> 26 THEN
        RAISE EXCEPTION 'task_complete_bulk: claim_token must be a 26-char ULID';
    END IF;

    -- Lock every target row in id order before taking a timestamp so a long
    -- lock wait cannot let an expired lease pass the fence.
    PERFORM 1
    FROM tasks
    WHERE id = ANY(p_task_ids)
    ORDER BY id
    FOR UPDATE;

    v_now := clock_timestamp();

    UPDATE tasks
    SET status = 'completed',
        completed_at = v_now,
        last_error = NULL,
        claim_token = NULL,
        claimed_by = NULL,
        lease_expires_at = NULL,
        updated_at = v_now
    WHERE id = ANY(p_task_ids)
      AND claim_token = p_claim_token
      AND status = 'running'
      AND lease_expires_at > v_now;
    GET DIAGNOSTICS v_updated = ROW_COUNT;
    RETURN v_updated;
END;
$$ LANGUAGE plpgsql;

-- Fail claimed tasks. Tasks that are retryable and still have attempt budget
-- return to pending with available_at = NOW() + retry_delay_seconds; the
-- rest become dead_letter. Returns the resulting status per task.
CREATE OR REPLACE FUNCTION task_fail_bulk(
    p_task_ids TEXT[],
    p_claim_token TEXT,
    p_error TEXT,
    p_retryable BOOLEAN,
    p_retry_delay_seconds INTEGER
) RETURNS TABLE (task_id TEXT, result_status TEXT) AS $$
DECLARE
    v_now TIMESTAMPTZ;
BEGIN
    IF p_task_ids IS NULL OR cardinality(p_task_ids) = 0 THEN
        RAISE EXCEPTION 'task_fail_bulk: task_ids must not be empty';
    END IF;
    IF p_claim_token IS NULL OR char_length(p_claim_token) <> 26 THEN
        RAISE EXCEPTION 'task_fail_bulk: claim_token must be a 26-char ULID';
    END IF;
    IF p_retryable IS NULL THEN
        RAISE EXCEPTION 'task_fail_bulk: retryable must be true or false';
    END IF;
    IF p_retry_delay_seconds IS NULL OR p_retry_delay_seconds < 0 THEN
        RAISE EXCEPTION 'task_fail_bulk: retry_delay_seconds must be >= 0';
    END IF;

    -- Lock every target row in id order before taking a timestamp so a long
    -- lock wait cannot let an expired lease pass the fence.
    PERFORM 1
    FROM tasks
    WHERE id = ANY(p_task_ids)
    ORDER BY id
    FOR UPDATE;

    v_now := clock_timestamp();

    RETURN QUERY
    UPDATE tasks t
    SET status = CASE
            WHEN p_retryable AND t.attempt_count < t.max_attempts THEN 'pending'
            ELSE 'dead_letter'
        END,
        available_at = CASE
            WHEN p_retryable AND t.attempt_count < t.max_attempts
                THEN v_now + make_interval(secs => p_retry_delay_seconds)
            ELSE t.available_at
        END,
        completed_at = CASE
            WHEN p_retryable AND t.attempt_count < t.max_attempts THEN NULL
            ELSE v_now
        END,
        last_error = p_error,
        claim_token = NULL,
        claimed_by = NULL,
        lease_expires_at = NULL,
        updated_at = v_now
    WHERE t.id = ANY(p_task_ids)
      AND t.claim_token = p_claim_token
      AND t.status = 'running'
      AND t.lease_expires_at > v_now
    RETURNING t.id AS task_id, t.status AS result_status;
END;
$$ LANGUAGE plpgsql;

-- Recover tasks whose lease expired while running. Tasks with remaining
-- attempt budget return to pending (immediately claimable); exhausted tasks
-- become dead_letter. Returns every recovered row so adapters that keep
-- domain state (e.g. agent_runs) can synchronize.
CREATE OR REPLACE FUNCTION task_recover_expired() RETURNS SETOF tasks AS $$
DECLARE
    v_now TIMESTAMPTZ;
BEGIN
    v_now := statement_timestamp();

    RETURN QUERY
    UPDATE tasks t
    SET status = CASE
            WHEN t.attempt_count < t.max_attempts THEN 'pending'
            ELSE 'dead_letter'
        END,
        available_at = CASE
            WHEN t.attempt_count < t.max_attempts THEN v_now
            ELSE t.available_at
        END,
        completed_at = CASE
            WHEN t.attempt_count < t.max_attempts THEN NULL
            ELSE v_now
        END,
        last_error = CASE
            WHEN t.attempt_count < t.max_attempts THEN 'task lease expired; requeued'
            ELSE 'task lease expired; retries exhausted'
        END,
        claim_token = NULL,
        claimed_by = NULL,
        lease_expires_at = NULL,
        updated_at = v_now
    WHERE t.status = 'running'
      AND t.lease_expires_at <= v_now
    RETURNING t.*;
END;
$$ LANGUAGE plpgsql;

-- Status statistics grouped by (task_type, status). Pass NULL for all types.
CREATE OR REPLACE FUNCTION task_stats(
    p_task_type TEXT
) RETURNS TABLE (task_type TEXT, status TEXT, count BIGINT) AS $$
BEGIN
    RETURN QUERY
    SELECT t.task_type::TEXT, t.status::TEXT, COUNT(*)::BIGINT
    FROM tasks t
    WHERE p_task_type IS NULL OR t.task_type = p_task_type
    GROUP BY t.task_type, t.status
    ORDER BY t.task_type, t.status;
END;
$$ LANGUAGE plpgsql;

-- Delete terminal (completed/dead_letter) tasks completed before the cutoff.
-- Returns the number of deleted rows.
CREATE OR REPLACE FUNCTION task_cleanup(
    p_before TIMESTAMPTZ
) RETURNS BIGINT AS $$
DECLARE
    v_deleted BIGINT;
BEGIN
    DELETE FROM tasks
    WHERE status IN ('completed', 'dead_letter')
      AND completed_at < p_before;
    GET DIAGNOSTICS v_deleted = ROW_COUNT;
    RETURN v_deleted;
END;
$$ LANGUAGE plpgsql;
