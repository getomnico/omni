-- Upgrade ParadeDB pg_search extension from 0.23.1 to 0.24.0.
--
-- 0.24.0 adds WAL-backed crash recovery for pg_search buffers. Reindex
-- BM25 indexes after the extension upgrade so existing indexes are rebuilt by
-- the upgraded extension code.

DO $$
DECLARE
    ext_ver TEXT;
    ext_parts TEXT[];
    ext_major INTEGER;
    ext_minor INTEGER;
    ext_patch INTEGER;
    index_name TEXT;
    bm25_indexes TEXT[] := ARRAY[
        'document_search_idx',
        'people_search_idx',
        'chat_message_content_search_idx',
        'chat_title_search_idx'
    ];
BEGIN
    SELECT extversion INTO ext_ver FROM pg_extension WHERE extname = 'pg_search';

    RAISE NOTICE 'pg_search current catalog version: %', ext_ver;

    IF ext_ver IS NULL THEN
        RAISE EXCEPTION 'pg_search extension is not installed';
    END IF;

    ext_parts := regexp_match(ext_ver, '^([0-9]+)\.([0-9]+)\.([0-9]+)');
    IF ext_parts IS NULL THEN
        RAISE EXCEPTION 'Unable to parse pg_search version %', ext_ver;
    END IF;

    ext_major := ext_parts[1]::INTEGER;
    ext_minor := ext_parts[2]::INTEGER;
    ext_patch := ext_parts[3]::INTEGER;

    IF (ext_major, ext_minor, ext_patch) >= (0, 24, 0) THEN
        RAISE NOTICE 'pg_search already at 0.24.0 or newer — nothing to upgrade';
        RETURN;
    END IF;

    ALTER EXTENSION pg_search UPDATE TO '0.24.0';

    FOREACH index_name IN ARRAY bm25_indexes LOOP
        IF to_regclass(index_name) IS NOT NULL THEN
            EXECUTE format('REINDEX INDEX %I', index_name);
        END IF;
    END LOOP;

    RAISE NOTICE 'pg_search upgraded to 0.24.0 — BM25 indexes rebuilt';
END;
$$;
