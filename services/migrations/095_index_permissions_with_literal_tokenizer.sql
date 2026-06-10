-- Rebuild the document BM25 index with literal-tokenized permissions.
--
-- Search permission checks use ParadeDB fielded queries against the indexed
-- JSONB permissions field (for example, users:alice@example.com and
-- groups:slack-channel\:T0\:C1). Literal tokenization preserves each user/group
-- value as an exact token, avoiding ACL leaks from tokenized partial matches
-- such as matching only an email localpart or domain.

DROP INDEX IF EXISTS document_search_idx;

CREATE INDEX document_search_idx ON documents
USING bm25 (
    id,
    (source_id::pdb.literal),
    (external_id::pdb.literal),
    (title::pdb.simple('ascii_folding=true')),
    (title::pdb.source_code('alias=title_secondary', 'ascii_folding=true')),
    (title::pdb.simple('alias=title_en', 'stemmer=english', 'ascii_folding=true')),
    (content::pdb.icu('ascii_folding=true')),
    (content::pdb.icu('alias=content_en', 'stemmer=english', 'ascii_folding=true')),
    (content_type::pdb.literal),
    file_size,
    (file_extension::pdb.literal),
    metadata,
    (permissions::pdb.literal),
    attributes,
    created_at,
    updated_at
)
WITH (
    key_field = id,
    background_layer_sizes = '100KB, 1MB, 10MB, 100MB, 1GB, 10GB',
    target_segment_count = 2,
    mutable_segment_rows = 100
);
