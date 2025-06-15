-- Test script for typo-tolerant search functionality

-- 1. Check if fuzzystrmatch extension is installed
SELECT * FROM pg_extension WHERE extname = 'fuzzystrmatch';

-- 2. Check if unique_lexemes materialized view exists
SELECT schemaname, matviewname 
FROM pg_matviews 
WHERE matviewname = 'unique_lexemes';

-- 3. View sample lexemes
SELECT word, ndoc, nentry 
FROM unique_lexemes 
ORDER BY ndoc DESC 
LIMIT 20;

-- 4. Test Levenshtein distance function
SELECT 
    'search' as correct_word,
    'serch' as typo,
    levenshtein('search', 'serch') as distance;

-- 5. Find similar words for a typo
SELECT word, levenshtein_less_equal(lower(word), lower('documnt'), 2) as distance
FROM unique_lexemes
WHERE levenshtein_less_equal(lower(word), lower('documnt'), 2) < 2
ORDER BY distance, ndoc DESC
LIMIT 5;

-- 6. Manual refresh of lexemes (if needed)
-- REFRESH MATERIALIZED VIEW CONCURRENTLY unique_lexemes;

-- 7. Check when lexemes were last refreshed
SELECT 
    schemaname,
    matviewname,
    last_refresh
FROM pg_stat_user_tables
WHERE schemaname = 'public' 
AND tablename = 'unique_lexemes';