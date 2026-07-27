-- Migration 106: Add W3C trace context columns to connector_events_queue and embedding_queue.
--
-- This migration adds nullable traceparent and tracestate columns to support
-- W3C TraceContext propagation through the asynchronous queue pipeline.
--
-- Columns:
--   traceparent (varchar(55)) — W3C traceparent header (version-format-trace_id-span_id-trace_flags)
--   tracestate  (varchar(512)) — optional W3C tracestate header (max 512 chars per spec)
--
-- Both columns are nullable; NULL means "no stored trace context".
-- Invalid/missing values are handled gracefully at read time.
-- No indexes: queue consumers read trace context after dequeue, not for filtering.

ALTER TABLE connector_events_queue ADD COLUMN IF NOT EXISTS traceparent varchar(55);
ALTER TABLE connector_events_queue ADD COLUMN IF NOT EXISTS tracestate varchar(512);

ALTER TABLE embedding_queue ADD COLUMN IF NOT EXISTS traceparent varchar(55);
ALTER TABLE embedding_queue ADD COLUMN IF NOT EXISTS tracestate varchar(512);
