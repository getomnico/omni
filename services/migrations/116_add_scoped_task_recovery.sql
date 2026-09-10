-- Generic type-scoped stale recovery for workers that own one task type.
-- Heartbeats update updated_at, so healthy long-running tasks are not recovered
-- merely because they have been running longer than the stale threshold.

DROP FUNCTION task_recover_expired_for_type(TEXT);

CREATE FUNCTION task_recover_stale_for_type(
    p_task_type TEXT,
    p_timeout_seconds INTEGER
) RETURNS SETOF tasks AS $$
DECLARE
    v_now TIMESTAMPTZ;
BEGIN
    IF p_task_type IS NULL OR btrim(p_task_type) = '' THEN
        RAISE EXCEPTION 'task_recover_stale_for_type: task_type must not be empty';
    END IF;
    IF p_timeout_seconds IS NULL OR p_timeout_seconds < 1 THEN
        RAISE EXCEPTION 'task_recover_stale_for_type: timeout_seconds must be >= 1';
    END IF;
    v_now := clock_timestamp();

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
      AND (
          t.lease_expires_at <= v_now
          OR t.updated_at <= v_now - make_interval(secs => p_timeout_seconds)
      )
    RETURNING t.*;
END;
$$ LANGUAGE plpgsql;
