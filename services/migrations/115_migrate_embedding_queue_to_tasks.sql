-- Add generic active-work deduplication and move document embedding work to tasks.
-- The tasks table remains workload-agnostic: document_id is carried only in the
-- versioned embedding payload and in the generic deduplication key.

ALTER TABLE tasks
    ADD COLUMN deduplication_key TEXT;

ALTER TABLE tasks
    ADD CONSTRAINT tasks_deduplication_key_check
    CHECK (deduplication_key IS NULL OR btrim(deduplication_key) <> '');

CREATE UNIQUE INDEX idx_tasks_active_deduplication_key
    ON tasks (task_type, deduplication_key)
    WHERE deduplication_key IS NOT NULL
      AND status IN ('pending', 'running');

COMMENT ON COLUMN tasks.deduplication_key IS
    'Optional opaque key that prevents duplicate pending or running work within a task type.';

-- Changing the argument list creates an overload in PostgreSQL. Remove the
-- migration-112 function first so an old nine-argument enqueue path cannot be
-- selected ambiguously by clients.
DROP FUNCTION task_enqueue_bulk(
    TEXT[], TEXT[], JSONB[], INTEGER[], INTEGER[], BIGINT[], BIGINT[], TEXT[], INTEGER[]
);

CREATE FUNCTION task_enqueue_bulk(
    p_ids TEXT[],
    p_task_types TEXT[],
    p_payloads JSONB[],
    p_payload_versions INTEGER[],
    p_priorities INTEGER[],
    p_available_at_ms BIGINT[],
    p_weights BIGINT[],
    p_concurrency_keys TEXT[],
    p_max_attempts INTEGER[],
    p_deduplication_keys TEXT[]
) RETURNS SETOF tasks AS $$
BEGIN
    RETURN QUERY
    INSERT INTO tasks (
        id, task_type, payload, payload_version,
        priority, available_at, weight, concurrency_key, max_attempts,
        deduplication_key
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
        ids.max_attempts,
        ids.deduplication_key
    FROM UNNEST(
        p_ids,
        p_task_types,
        p_payloads,
        p_payload_versions,
        p_priorities,
        p_available_at_ms,
        p_weights,
        p_concurrency_keys,
        p_max_attempts,
        p_deduplication_keys
    ) AS ids(
        id, task_type, payload, payload_version,
        priority, available_at_ms, weight, concurrency_key, max_attempts,
        deduplication_key
    )
    ON CONFLICT DO NOTHING
    RETURNING *;
END;
$$ LANGUAGE plpgsql;

-- A scoped form is used by adapters that own one workload. Keeping the
-- unscoped function from migration 112 preserves the generic recovery API.
CREATE FUNCTION task_recover_expired_for_type(p_task_type TEXT)
RETURNS SETOF tasks AS $$
DECLARE
    v_now TIMESTAMPTZ;
BEGIN
    IF p_task_type IS NULL OR btrim(p_task_type) = '' THEN
        RAISE EXCEPTION 'task_recover_expired_for_type: task_type must not be empty';
    END IF;
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
    WHERE t.task_type = p_task_type
      AND t.status = 'running'
      AND t.lease_expires_at <= v_now
    RETURNING t.*;
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION task_cleanup_for_type(
    p_task_type TEXT,
    p_before TIMESTAMPTZ
) RETURNS BIGINT AS $$
DECLARE
    v_deleted BIGINT;
BEGIN
    IF p_task_type IS NULL OR btrim(p_task_type) = '' THEN
        RAISE EXCEPTION 'task_cleanup_for_type: task_type must not be empty';
    END IF;
    DELETE FROM tasks
    WHERE task_type = p_task_type
      AND status IN ('completed', 'dead_letter')
      AND completed_at < p_before;
    GET DIAGNOSTICS v_deleted = ROW_COUNT;
    RETURN v_deleted;
END;
$$ LANGUAGE plpgsql;

-- Backfill deterministically. Processing rows cannot retain their old worker
-- ownership, so they are requeued. Failed rows with five or more failures are
-- represented as dead letters; all other rows retain their failure count as
-- attempt_count. Only the oldest active row per document is retained because
-- the new generic active deduplication index intentionally rejects duplicates.
WITH mapped AS (
    SELECT
        CASE
            WHEN char_length(q.id) = 26 THEN q.id
            ELSE substring(md5('embedding_queue:' || q.id), 1, 26)
        END AS task_id,
        q.document_id,
        q.status AS legacy_status,
        q.retry_count,
        q.error_message,
        q.created_at,
        q.updated_at,
        q.processed_at,
        CASE
            WHEN q.status = 'completed' THEN 'completed'
            WHEN q.status = 'failed' AND q.retry_count >= 5 THEN 'dead_letter'
            ELSE 'pending'
        END AS task_status,
        LEAST(GREATEST(q.retry_count, 0), 5) AS attempt_count
    FROM embedding_queue q
), ranked AS (
    SELECT mapped.*,
           row_number() OVER (
               PARTITION BY document_id
               ORDER BY CASE WHEN task_status = 'pending' THEN 0 ELSE 1 END,
                        created_at, task_id
           ) AS active_rank
    FROM mapped
)
INSERT INTO tasks (
    id, task_type, payload, payload_version, status,
    priority, available_at, weight, max_attempts, attempt_count,
    last_error, created_at, updated_at, completed_at, deduplication_key
)
SELECT
    r.task_id,
    'document_embedding',
    jsonb_build_object('document_id', r.document_id),
    1,
    r.task_status,
    0,
    CASE
        WHEN r.task_status = 'pending' THEN COALESCE(r.updated_at, r.created_at, NOW())
        ELSE COALESCE(r.created_at, NOW())
    END,
    1,
    5,
    r.attempt_count,
    CASE
        WHEN r.legacy_status = 'failed' THEN COALESCE(r.error_message, 'migrated from embedding_queue')
        WHEN r.legacy_status = 'processing' THEN 'migrated from processing embedding_queue item'
        ELSE NULL
    END,
    COALESCE(r.created_at, NOW()),
    COALESCE(r.updated_at, r.created_at, NOW()),
    CASE
        WHEN r.task_status IN ('completed', 'dead_letter')
            THEN COALESCE(r.processed_at, r.updated_at, r.created_at, NOW())
        ELSE NULL
    END,
    r.document_id
FROM ranked r
WHERE r.task_status <> 'pending' OR r.active_rank = 1
ON CONFLICT DO NOTHING;

DROP TABLE embedding_queue;
DROP FUNCTION IF EXISTS notify_embedding_queue();
