-- Drop the lexeme refresh function
DROP FUNCTION IF EXISTS refresh_unique_lexemes();

-- Drop the materialized view and its indexes
DROP MATERIALIZED VIEW IF EXISTS unique_lexemes;

-- Drop the fuzzystrmatch extension
-- Note: Only drop if no other functionality depends on it
DROP EXTENSION IF EXISTS fuzzystrmatch;
