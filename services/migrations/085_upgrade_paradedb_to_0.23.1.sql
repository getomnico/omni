-- Upgrade ParadeDB pg_search extension from 0.20.6 to 0.23.1.
--
-- Handles both fresh installs (extension already at 0.23.1 — no-op) and
-- upgrades from 0.20.6 (ALTER EXTENSION + recreate ICU-backed index that
-- the upgrade scripts drop).
--
-- 0.22.0 reimplemented the ICU tokenizer from ICU4C to Rust-native
-- icu_segmenter, which invalidates any index using pdb.icu. Only
-- document_search_idx is affected; the other three BM25 indexes survive.

DO $$
DECLARE
    ext_ver TEXT;
BEGIN
    SELECT extversion INTO ext_ver FROM pg_extension WHERE extname = 'pg_search';

    RAISE NOTICE 'pg_search current catalog version: %', ext_ver;

    IF ext_ver = '0.23.1' THEN
        RAISE NOTICE 'pg_search already at 0.23.1 — nothing to upgrade';
        RETURN;
    END IF;

    IF ext_ver != '0.20.6' THEN
        RAISE EXCEPTION 'pg_search at unexpected version %. Expected 0.20.6 or 0.23.1.',
            ext_ver;
    END IF;

    -- Upgrade from 0.20.6
    ALTER EXTENSION pg_search UPDATE TO '0.23.1';

    -- Recreate the ICU-backed document index (dropped by the upgrade scripts)
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
        file_size, file_extension, metadata, permissions, attributes, created_at, updated_at
    )
    WITH (
        key_field = id,
        background_layer_sizes = '100KB, 1MB, 10MB, 100MB, 1GB, 10GB',
        target_segment_count = 2,
        mutable_segment_rows = 5000
    );

    -- Rebuild surviving indexes for new tokenizer internals
    REINDEX INDEX people_search_idx;
    REINDEX INDEX chat_message_content_search_idx;
    REINDEX INDEX chat_title_search_idx;

    RAISE NOTICE 'pg_search upgraded to 0.23.1 — all BM25 indexes rebuilt';

    -- Silence collation version mismatch after OS/libc bump between images
    EXECUTE format('ALTER DATABASE %I REFRESH COLLATION VERSION', current_database());
END;
$$;
