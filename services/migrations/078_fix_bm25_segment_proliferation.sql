-- Fix BM25 index segment proliferation that causes search corruption.
--
-- Root cause: mutable_segment_rows = 0 (set in 041) forces every batch write
-- to create a new immutable segment. With 128-doc batches every few seconds,
-- this creates 40+ segments that overwhelm the background merger. Queries
-- reading segment metadata during stale/in-progress merges hit:
--   "UUID parsing failed: invalid length: expected 16 bytes, found 0"
--
-- Fix: re-enable mutable segment buffering (5000 rows) so writes accumulate
-- in memory before flushing to immutable segments. This dramatically reduces
-- segment count and keeps the background merger ahead.
--
-- After applying this migration, run: REINDEX INDEX document_search_idx;

ALTER INDEX document_search_idx SET (mutable_segment_rows = 5000);
