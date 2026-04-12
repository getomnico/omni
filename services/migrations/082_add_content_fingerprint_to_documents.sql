-- Add content_fingerprint column for cross-source deduplication.
-- Connectors compute a fingerprint from source-independent identifiers
-- (e.g. RFC 2822 Message-ID for email threads) so the indexer and AI
-- service can detect semantically identical documents across sources
-- without relying on content hashing (which fails when bodies differ
-- due to mailing-list footers, DMARC From-rewriting, per-account flags, etc.).

ALTER TABLE documents ADD COLUMN IF NOT EXISTS content_fingerprint VARCHAR(128);
CREATE INDEX IF NOT EXISTS idx_documents_content_fingerprint ON documents(content_fingerprint) WHERE content_fingerprint IS NOT NULL;
