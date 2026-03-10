-- Multilingual FTS via dual tokenizers + English-stemmed aliases.
--
-- Title fields use pdb.simple: splits on non-alphanumeric characters (underscores,
--   dots, hyphens), lowercases. Handles filenames like
--   "Quartalsbericht_Q3_2024_Entwurf.docx" correctly across any language.
--
-- Content fields use pdb.icu: Unicode word-boundary segmentation — handles CJK,
--   Thai, and other scripts that don't use spaces between words. For space-
--   separated languages (English, German, etc.) it produces the same result as
--   simple.
--
-- English aliases (title_en, content_en): same base tokenizer + Snowball English
--   stemmer. Preserves English morphological matching ("reports" → "report")
--   without degrading other languages — the primary unstemmed path is always
--   searched alongside.
--
-- source_code alias (title_secondary): CamelCase splitting for code identifiers.
--
-- No stopwords: no universal list exists; BM25/IDF naturally downweights
-- high-frequency terms. English stopwords would wrongly remove words in other
-- languages (e.g. "die" is a German article).
--
-- ASCII folding kept on all paths for typing convenience (ä→a, ü→u, ñ→n).

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

-- Re-apply tuning from migration 058 so this migration is self-contained
ALTER INDEX document_search_idx SET (mutable_segment_rows = 0);
ALTER INDEX document_search_idx SET (target_segment_count = 2);
