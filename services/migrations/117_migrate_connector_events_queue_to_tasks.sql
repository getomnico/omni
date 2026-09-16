-- Move connector event delivery onto the canonical task queue. This migration
-- is intentionally a single cutover: the connector manager and indexer must be
-- deployed after it has committed.

-- ---------------------------------------------------------------------------
-- Generic lifecycle operations needed by bulk connector workers.
-- ---------------------------------------------------------------------------

CREATE FUNCTION task_heartbeat_bulk(
    p_task_ids TEXT[],
    p_claim_token TEXT,
    p_lease_seconds INTEGER
) RETURNS TABLE (task_id TEXT) AS $$
DECLARE
    v_now TIMESTAMPTZ;
BEGIN
    IF p_task_ids IS NULL OR cardinality(p_task_ids) = 0 THEN
        RAISE EXCEPTION 'task_heartbeat_bulk: task_ids must not be empty';
    END IF;
    IF p_claim_token IS NULL OR char_length(p_claim_token) <> 26 THEN
        RAISE EXCEPTION 'task_heartbeat_bulk: claim_token must be a 26-char ULID';
    END IF;
    IF p_lease_seconds IS NULL OR p_lease_seconds < 1 THEN
        RAISE EXCEPTION 'task_heartbeat_bulk: lease_seconds must be >= 1';
    END IF;
    IF EXISTS (
        SELECT 1 FROM UNNEST(p_task_ids) AS ids(id)
        WHERE id IS NULL OR char_length(id) <> 26
    ) THEN
        RAISE EXCEPTION 'task_heartbeat_bulk: every task id must be a 26-char ULID';
    END IF;

    PERFORM 1 FROM tasks WHERE id = ANY(p_task_ids) ORDER BY id FOR UPDATE;
    v_now := clock_timestamp();

    RETURN QUERY
    UPDATE tasks t
    SET lease_expires_at = v_now + make_interval(secs => p_lease_seconds),
        updated_at = v_now
    WHERE t.id = ANY(p_task_ids)
      AND t.claim_token = p_claim_token
      AND t.status = 'running'
      AND t.lease_expires_at > v_now
    RETURNING t.id;
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION task_fail_bulk_with_errors(
    p_task_ids TEXT[],
    p_errors TEXT[],
    p_claim_token TEXT,
    p_retryable BOOLEAN,
    p_retry_delay_seconds INTEGER
) RETURNS TABLE (task_id TEXT, result_status TEXT) AS $$
DECLARE
    v_now TIMESTAMPTZ;
BEGIN
    IF p_task_ids IS NULL OR cardinality(p_task_ids) = 0 THEN
        RAISE EXCEPTION 'task_fail_bulk_with_errors: task_ids must not be empty';
    END IF;
    IF p_errors IS NULL OR cardinality(p_errors) <> cardinality(p_task_ids) THEN
        RAISE EXCEPTION 'task_fail_bulk_with_errors: task_ids and errors must have equal lengths';
    END IF;
    IF p_claim_token IS NULL OR char_length(p_claim_token) <> 26 THEN
        RAISE EXCEPTION 'task_fail_bulk_with_errors: claim_token must be a 26-char ULID';
    END IF;
    IF p_retryable IS NULL THEN
        RAISE EXCEPTION 'task_fail_bulk_with_errors: retryable must be true or false';
    END IF;
    IF p_retry_delay_seconds IS NULL OR p_retry_delay_seconds < 0 THEN
        RAISE EXCEPTION 'task_fail_bulk_with_errors: retry_delay_seconds must be >= 0';
    END IF;
    IF EXISTS (
        SELECT 1 FROM UNNEST(p_task_ids) AS ids(id)
        WHERE id IS NULL OR char_length(id) <> 26
    ) THEN
        RAISE EXCEPTION 'task_fail_bulk_with_errors: every task id must be a 26-char ULID';
    END IF;

    PERFORM 1 FROM tasks WHERE id = ANY(p_task_ids) ORDER BY id FOR UPDATE;
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
        last_error = errors.error_message,
        claim_token = NULL,
        claimed_by = NULL,
        lease_expires_at = NULL,
        updated_at = v_now
    FROM UNNEST(p_task_ids, p_errors) AS errors(id, error_message)
    WHERE t.id = errors.id
      AND t.claim_token = p_claim_token
      AND t.status = 'running'
      AND t.lease_expires_at > v_now
    RETURNING t.id, t.status;
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION task_dead_letter_pending(
    p_task_ids TEXT[],
    p_reason TEXT
) RETURNS TABLE (task_id TEXT) AS $$
DECLARE
    v_now TIMESTAMPTZ;
BEGIN
    IF p_task_ids IS NULL OR cardinality(p_task_ids) = 0 THEN
        RAISE EXCEPTION 'task_dead_letter_pending: task_ids must not be empty';
    END IF;
    IF p_reason IS NULL OR btrim(p_reason) = '' THEN
        RAISE EXCEPTION 'task_dead_letter_pending: reason must not be empty';
    END IF;
    IF EXISTS (
        SELECT 1 FROM UNNEST(p_task_ids) AS ids(id)
        WHERE id IS NULL OR char_length(id) <> 26
    ) THEN
        RAISE EXCEPTION 'task_dead_letter_pending: every task id must be a 26-char ULID';
    END IF;

    PERFORM 1 FROM tasks WHERE id = ANY(p_task_ids) ORDER BY id FOR UPDATE;
    v_now := clock_timestamp();

    RETURN QUERY
    UPDATE tasks t
    SET status = 'dead_letter',
        completed_at = v_now,
        last_error = p_reason,
        claim_token = NULL,
        claimed_by = NULL,
        lease_expires_at = NULL,
        updated_at = v_now
    WHERE t.id = ANY(p_task_ids)
      AND t.status = 'pending'
      AND t.attempt_count > 0
    RETURNING t.id;
END;
$$ LANGUAGE plpgsql;

-- A generic task may already use a legacy connector ULID. Never silently
-- convert such a collision into a no-op: that would lose an emitted event.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM connector_events_queue legacy
        JOIN tasks existing ON existing.id = legacy.id
        WHERE existing.task_type <> 'connector_event'
    ) THEN
        RAISE EXCEPTION 'connector event task id collides with a non-connector task';
    END IF;

    IF EXISTS (
        SELECT 1 FROM connector_events_queue
        WHERE status NOT IN ('pending', 'processing', 'failed', 'completed', 'dead_letter')
    ) THEN
        RAISE EXCEPTION 'unexpected connector event queue status during migration';
    END IF;
END;
$$;

-- Backfill every event. Failed rows whose retry window has elapsed are
-- terminal; active retryable failures retain their consumed attempt budget.
INSERT INTO tasks (
    id, task_type, payload, payload_version, status, priority, available_at,
    weight, concurrency_key, attempt_count, max_attempts, last_error,
    created_at, updated_at, last_started_at, completed_at
)
SELECT
    q.id,
    'connector_event',
    q.payload,
    1,
    CASE
        WHEN q.status = 'completed' THEN 'completed'
        WHEN q.status = 'dead_letter' THEN 'dead_letter'
        WHEN q.status = 'failed'
             AND (COALESCE(q.retry_count, 0) >= COALESCE(q.max_retries, 3)
                  OR q.created_at <= NOW() - INTERVAL '24 hours') THEN 'dead_letter'
        ELSE 'pending'
    END,
    0,
    CASE
        WHEN q.status IN ('pending', 'processing') THEN NOW()
        WHEN q.status = 'failed'
             THEN GREATEST(COALESCE(q.processed_at, q.processing_started_at, q.created_at, NOW()) + INTERVAL '5 minutes', NOW())
        ELSE COALESCE(q.created_at, NOW())
    END,
    octet_length(q.payload::text)::BIGINT + COALESCE(cb.size_bytes, 0),
    CASE q.payload ->> 'type'
        WHEN 'person_sync' THEN q.payload ->> 'source_id' || ':' || lower(btrim(q.payload #>> '{person,email}'))
        WHEN 'person_deleted' THEN q.payload ->> 'source_id' || ':' || lower(btrim(q.payload ->> 'email'))
        ELSE NULL
    END,
    LEAST(
        GREATEST(
            COALESCE(q.retry_count, 0),
            0
        ),
        GREATEST(COALESCE(q.max_retries, 3), 1)
    ),
    GREATEST(COALESCE(q.max_retries, 3), 1),
    CASE
        WHEN q.status IN ('failed', 'dead_letter') THEN COALESCE(q.error_message, 'migrated from connector_events_queue')
        ELSE NULL
    END,
    COALESCE(q.created_at, NOW()),
    COALESCE(q.processed_at, q.processing_started_at, q.created_at, NOW()),
    CASE WHEN q.status = 'processing' THEN COALESCE(q.processing_started_at, q.created_at) ELSE NULL END,
    CASE
        WHEN q.status = 'completed'
             OR q.status = 'dead_letter'
             OR (q.status = 'failed'
                 AND (COALESCE(q.retry_count, 0) >= COALESCE(q.max_retries, 3)
                      OR q.created_at <= NOW() - INTERVAL '24 hours'))
            THEN COALESCE(q.processed_at, q.processing_started_at, q.created_at, NOW())
        ELSE NULL
    END
FROM connector_events_queue q
LEFT JOIN content_blobs cb
  ON char_length(q.payload ->> 'content_id') = 26
 AND cb.id = (q.payload ->> 'content_id')::char(26);

-- Workload-specific indexes used by candidate selection and content GC.
CREATE INDEX idx_tasks_connector_source
    ON tasks ((payload ->> 'source_id'), id)
    WHERE task_type = 'connector_event';
CREATE INDEX idx_tasks_connector_sync_run
    ON tasks ((payload ->> 'sync_run_id'), id)
    WHERE task_type = 'connector_event';
CREATE INDEX idx_tasks_connector_event_type
    ON tasks ((payload ->> 'type'), id)
    WHERE task_type = 'connector_event';
CREATE INDEX idx_tasks_connector_content_id
    ON tasks ((payload ->> 'content_id'))
    WHERE task_type = 'connector_event'
      AND status IN ('pending', 'running')
      AND payload ->> 'content_id' IS NOT NULL;
CREATE INDEX idx_tasks_connector_person_identity_order
    ON tasks ((payload ->> 'source_id'),
              lower(btrim(CASE payload ->> 'type'
                  WHEN 'person_sync' THEN payload #>> '{person,email}'
                  ELSE payload ->> 'email'
              END)), id)
    WHERE task_type = 'connector_event'
      AND payload ->> 'type' IN ('person_sync', 'person_deleted')
      AND status IN ('pending', 'running');

-- Keep the cutover invariant executable while the legacy table still exists.
-- In particular, deployment interruption must not turn a processing claim
-- into a consumed attempt or a dead letter.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM connector_events_queue legacy
        JOIN tasks migrated ON migrated.id = legacy.id
        WHERE legacy.status = 'processing'
          AND (
              migrated.status <> 'pending'
              OR migrated.completed_at IS NOT NULL
              OR migrated.attempt_count <> LEAST(
                  GREATEST(COALESCE(legacy.retry_count, 0), 0),
                  GREATEST(COALESCE(legacy.max_retries, 3), 1)
              )
          )
    ) THEN
        RAISE EXCEPTION 'processing connector event was not requeued without consuming an attempt';
    END IF;
END;
$$;

DROP INDEX IF EXISTS idx_connector_events_person_identity_order;
DROP INDEX IF EXISTS idx_queue_status_created;
DROP INDEX IF EXISTS idx_queue_source_id;
DROP INDEX IF EXISTS idx_queue_status;
DROP INDEX IF EXISTS idx_queue_retry;
DROP INDEX IF EXISTS idx_queue_sync_run_id;
DROP INDEX IF EXISTS idx_connector_events_queue_processing_stale;
DROP TABLE connector_events_queue;
