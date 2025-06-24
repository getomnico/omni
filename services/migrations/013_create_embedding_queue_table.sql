-- Create embedding queue table for async embedding generation
CREATE TABLE IF NOT EXISTS embedding_queue (
    id TEXT PRIMARY KEY,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    content TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'processing', 'completed', 'failed')),
    retry_count INTEGER NOT NULL DEFAULT 0,
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    processed_at TIMESTAMPTZ
);

-- Create indexes for efficient querying
CREATE INDEX idx_embedding_queue_status ON embedding_queue(status);
CREATE INDEX idx_embedding_queue_status_created ON embedding_queue(status, created_at);
CREATE INDEX idx_embedding_queue_document_id ON embedding_queue(document_id);

-- Add embedding status to documents table
ALTER TABLE documents ADD COLUMN IF NOT EXISTS embedding_status TEXT DEFAULT 'pending' CHECK (embedding_status IN ('pending', 'processing', 'completed', 'failed'));
CREATE INDEX idx_documents_embedding_status ON documents(embedding_status);

-- Create notification channel for embedding queue
CREATE OR REPLACE FUNCTION notify_embedding_queue() RETURNS trigger AS $$
BEGIN
    IF NEW.status = 'pending' THEN
        PERFORM pg_notify('embedding_queue', json_build_object('id', NEW.id)::text);
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER embedding_queue_notify
    AFTER INSERT OR UPDATE ON embedding_queue
    FOR EACH ROW
    EXECUTE FUNCTION notify_embedding_queue();