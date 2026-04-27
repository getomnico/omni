-- Upgrade ParadeDB pg_search extension from 0.20.6 to 0.23.1.
--
-- Fixes background merger bugs (segments not consolidating, deleted docs
-- accumulating) that cause query performance degradation (issue #209).
--
-- Also switches the ICU tokenizer from ICU4C to the Rust-native icu_segmenter
-- crate (changed in ParadeDB 0.22.0). REINDEX rebuilds all BM25 segments
-- against the new tokenizer for consistent search results.
--
-- IMPORTANT: This migration must ship together with the Docker image bump
-- to paradedb/paradedb:0.23.1-pg17. Running this migration on the old
-- 0.20.6 image will fail because the 0.23.1 .so library won't be present.

ALTER EXTENSION pg_search UPDATE TO '0.23.1';

-- Rebuild all BM25 indexes against the new extension internals.
-- document_search_idx uses pdb.icu, whose tokenizer implementation switched
-- from ICU4C to Rust-native icu_segmenter in 0.22.0 — rebuild is required.
-- The other three indexes use tokenizers that haven't changed (pdb.simple,
-- pdb.ngram, default) but are reindexed for consistency.
-- Uses plain REINDEX (not CONCURRENTLY) because sqlx runs migrations in a
-- transaction, and CONCURRENTLY cannot execute inside a transaction block.
REINDEX INDEX document_search_idx;
REINDEX INDEX people_search_idx;
REINDEX INDEX chat_message_content_search_idx;
REINDEX INDEX chat_title_search_idx;
