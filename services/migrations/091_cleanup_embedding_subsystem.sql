-- Cleanup of two unused embedding-subsystem schema artifacts.
--
-- 1. Bedrock batch inference: We'll revisit cloud batch inference with a better design later.
-- 2. documents.embedding_status. Documents table should not have embedding status.

DROP INDEX IF EXISTS idx_embedding_queue_batch_job;
DROP INDEX IF EXISTS idx_embedding_queue_pending_no_batch;
ALTER TABLE embedding_queue DROP COLUMN IF EXISTS batch_job_id;
DROP TABLE IF EXISTS embedding_batch_jobs;

DROP INDEX IF EXISTS idx_documents_embedding_status;
ALTER TABLE documents DROP COLUMN IF EXISTS embedding_status;
