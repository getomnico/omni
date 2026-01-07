-- ParadeDB v0.20.0+ syntax migration
-- ParadeDB does not support indexing CHAR(n) columns, so we update the types
ALTER TABLE documents ALTER COLUMN id TYPE VARCHAR(26);
ALTER TABLE documents ALTER COLUMN source_id TYPE VARCHAR(26);
ALTER TABLE documents ALTER COLUMN content_id TYPE VARCHAR(26);

CREATE INDEX document_search_idx ON documents
USING bm25 (
    id,
    (source_id::pdb.literal),
    (external_id::pdb.literal),
    (title::pdb.ngram(2, 3)),
    (content::pdb.ngram(2, 3)),
    (content_type::pdb.literal),
    file_size,
    file_extension,
    metadata,
    permissions,
    created_at,
    updated_at,
    last_indexed_at
)
WITH (key_field = 'id');
