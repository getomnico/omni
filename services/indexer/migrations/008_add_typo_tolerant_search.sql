-- Enable fuzzystrmatch extension for Levenshtein distance functions
CREATE EXTENSION IF NOT EXISTS fuzzystrmatch;

-- Create materialized view for unique lexemes from all documents
CREATE MATERIALIZED VIEW IF NOT EXISTS unique_lexemes AS
SELECT word, ndoc, nentry
FROM ts_stat('SELECT tsv_content FROM documents')
WHERE length(word) >= 3  -- Only include words with 3+ characters
ORDER BY ndoc DESC;

-- Create indexes for efficient lookups
CREATE INDEX IF NOT EXISTS idx_unique_lexemes_word ON unique_lexemes(word);
CREATE INDEX IF NOT EXISTS idx_unique_lexemes_word_lower ON unique_lexemes(lower(word));

-- Create function to refresh the materialized view
CREATE OR REPLACE FUNCTION refresh_unique_lexemes()
RETURNS void AS $$
BEGIN
    REFRESH MATERIALIZED VIEW CONCURRENTLY unique_lexemes;
END;
$$ LANGUAGE plpgsql;

-- Add comment explaining the purpose
COMMENT ON MATERIALIZED VIEW unique_lexemes IS 'Stores unique words from all documents for typo-tolerant search';