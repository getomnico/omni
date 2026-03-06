-- Match target_segment_count to max_parallel_workers_per_gather (default=2)
-- Takes effect on next REINDEX
ALTER INDEX document_search_idx SET (target_segment_count = 2);

-- Configure aggressive autovacuum for the documents table
-- ParadeDB recommends vacuuming at least every 100k single-row updates
-- to keep the visibility map fresh and avoid excessive heap fetches
ALTER TABLE documents SET (
    autovacuum_vacuum_scale_factor = 0,
    autovacuum_vacuum_threshold = 10000
);
