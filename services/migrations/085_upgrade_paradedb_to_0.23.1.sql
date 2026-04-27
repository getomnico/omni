-- Upgrade ParadeDB pg_search extension from 0.20.6 to 0.23.1.
--
-- Fixes background merger bugs (segments not consolidating, deleted docs
-- accumulating) that cause query performance degradation (issue #209).
--
-- Also switches the ICU tokenizer from ICU4C to the Rust-native icu_segmenter
-- crate (changed in ParadeDB 0.22.0). REINDEX rebuilds all BM25 segments
-- against the new tokenizer for consistent search results.

ALTER EXTENSION pg_search UPDATE TO '0.23.1';

-- Rebuild the BM25 index against the new tokenizer internals.
REINDEX INDEX document_search_idx;
