DROP INDEX IF EXISTS document_search_idx;

CREATE INDEX document_search_idx ON documents
USING bm25 (
    id,
    (source_id::pdb.literal),
    (external_id::pdb.literal),
    (title::pdb.simple('stemmer=english', 'ascii_folding=true', 'stopwords_language=english')),
    (title::pdb.source_code('alias=title_secondary', 'stemmer=english', 'ascii_folding=true', 'stopwords_language=english')),
    (content::pdb.simple('stemmer=english', 'ascii_folding=true', 'stopwords_language=english')),
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

ALTER INDEX document_search_idx SET (mutable_segment_rows = 0);
