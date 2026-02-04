-- Remove last_indexed_at from BM25 index and fix syntax
-- Migration 036 used old ParadeDB syntax, this uses the new v0.20.0+ syntax
-- last_indexed_at is not used for search filtering and causes double index writes

DROP INDEX IF EXISTS document_search_idx;

CREATE INDEX document_search_idx ON documents
USING bm25 (
    id,
    (source_id::pdb.literal),
    (external_id::pdb.literal),
    (title::pdb.ngram(2, 3)),
    content,
    (content_type::pdb.literal),
    file_size,
    file_extension,
    metadata,
    permissions,
    attributes,
    created_at,
    updated_at
)
WITH (
    key_field = 'id',
    background_layer_sizes = '100KB, 1MB, 10MB, 100MB, 1GB, 10GB'
);
