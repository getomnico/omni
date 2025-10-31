
-- ParadeDB does not support indexing CHAR(n) columns, so we update the types
ALTER TABLE documents ALTER COLUMN id TYPE VARCHAR(26);
ALTER TABLE documents ALTER COLUMN source_id TYPE VARCHAR(26);
ALTER TABLE documents ALTER COLUMN content_id TYPE VARCHAR(26);

CREATE INDEX document_search_idx ON documents
USING bm25 (id, source_id, external_id, title, content, content_type, file_size, file_extension, metadata, permissions, created_at, updated_at, last_indexed_at)
WITH (
    key_field = 'id',
    text_fields = '{
        "source_id": { "fast": true, "tokenizer": { "type": "keyword" } },
        "external_id": { "fast": true, "tokenizer": { "type": "keyword" } },
        "content_type": { "fast": true, "tokenizer": { "type": "keyword" } },
        "title": {
            "tokenizer": {
                "type": "ngram",
                "min_gram": 2,
                "max_gram": 3,
                "prefix_only": false
            }
        },
        "content": {
            "tokenizer": {
                "type": "ngram",
                "min_gram": 2,
                "max_gram": 3,
                "prefix_only": false
            }
        }
    }',
    json_fields = '{
        "metadata": {
            "tokenizer": {
                "type": "keyword"
            }
        },
        "permissions": {
            "tokenizer": {
                "type": "keyword"
            }
        }
    }'
);
