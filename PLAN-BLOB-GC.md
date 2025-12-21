# Plan: Content Blob Garbage Collection

## Problem Statement

Connectors create new content blobs on each sync when content changes. When a document is updated, a new `content_blob` is created and the document's `content_id` is updated to point to it. However, the old content blob is never deleted, leading to unbounded storage growth.

**Orphaned blobs occur when:**
1. Document updates replace `content_id` with a new blob
2. Documents are deleted (content_blobs remain due to `ON DELETE SET NULL`)
3. Indexing fails after content storage but before document creation

## Current Architecture

### Data Model
- `content_blobs` table: stores content with `id`, `content`, `size_bytes`, `sha256_hash`, `created_at`
- `documents` table: references `content_blobs` via `content_id REFERENCES content_blobs(id) ON DELETE SET NULL`
- Relationship: Many documents can reference one blob (though typically 1:1 in practice)

### Existing Cleanup Infrastructure
The indexer's `QueueProcessor` (`services/indexer/src/queue_processor.rs:217-237`) already runs hourly cleanup tasks:
- `event_queue.cleanup_old_events(7)` - deletes completed/dead_letter events older than 7 days
- `embedding_queue.cleanup_completed(7)` - deletes completed embedding items older than 7 days

### Storage Layer
- `ObjectStorage` trait (`shared/src/storage/mod.rs:28-82`) already has `delete_content()` method
- PostgreSQL implementation (`shared/src/storage/postgres.rs:73-86`) implements deletion

## Implementation Plan

### Step 1: Add GC Method to Storage Layer

**File:** `shared/src/storage/postgres.rs`

Add a new method to find and delete orphaned content blobs:

```rust
/// Garbage collect orphaned content blobs that are no longer referenced by any document.
/// Returns the number of blobs deleted and total bytes reclaimed.
pub async fn gc_orphaned_blobs(&self, retention_days: i32, batch_size: i64) -> Result<GcResult, StorageError> {
    // Find orphaned blobs older than retention period, limited to batch_size
    // Delete them in a transaction
    // Return statistics
}
```

**Query to find orphaned blobs:**
```sql
SELECT cb.id, cb.size_bytes
FROM content_blobs cb
LEFT JOIN documents d ON d.content_id = cb.id
WHERE d.id IS NULL
  AND cb.created_at < NOW() - INTERVAL '1 day' * $1
ORDER BY cb.created_at ASC
LIMIT $2
```

**Key design decisions:**
- **Retention period (7 days):** Grace period to avoid deleting blobs for in-flight operations
- **Batch size (1000):** Limit deletions per run to avoid long-running transactions
- **Order by created_at ASC:** Delete oldest orphans first

### Step 2: Add GC Result Type

**File:** `shared/src/storage/mod.rs`

```rust
#[derive(Debug, Default)]
pub struct GcResult {
    pub blobs_deleted: u64,
    pub bytes_reclaimed: u64,
}
```

### Step 3: Add Trait Method to ObjectStorage

**File:** `shared/src/storage/mod.rs`

Add to the `ObjectStorage` trait:
```rust
async fn gc_orphaned_blobs(&self, retention_days: i32, batch_size: i64) -> Result<GcResult, StorageError>;
```

### Step 4: Implement for S3 Storage Backend

**File:** `shared/src/storage/s3.rs`

For S3 backend, orphaned blobs are tracked in the `content_blobs` table (metadata only), with actual content in S3. The GC needs to:
1. Find orphaned rows in `content_blobs` where `storage_backend = 's3'`
2. Delete the S3 object using `storage_key`
3. Delete the database row

### Step 5: Integrate into QueueProcessor

**File:** `services/indexer/src/queue_processor.rs`

Add a new timer alongside existing cleanup timers (~line 168):
```rust
let mut blob_gc_interval = interval(Duration::from_secs(3600)); // 1 hour
```

Add handler in the select! loop (~line 217):
```rust
_ = blob_gc_interval.tick() => {
    match self.state.content_storage.gc_orphaned_blobs(7, 1000).await {
        Ok(result) => {
            if result.blobs_deleted > 0 {
                info!(
                    "Content blob GC: deleted {} orphaned blobs, reclaimed {} bytes",
                    result.blobs_deleted,
                    result.bytes_reclaimed
                );
            }
        }
        Err(e) => {
            error!("Content blob GC failed: {}", e);
        }
    }
}
```

## File Changes Summary

| File | Change |
|------|--------|
| `shared/src/storage/mod.rs` | Add `GcResult` struct and `gc_orphaned_blobs` trait method |
| `shared/src/storage/postgres.rs` | Implement `gc_orphaned_blobs` for PostgreSQL |
| `shared/src/storage/s3.rs` | Implement `gc_orphaned_blobs` for S3 |
| `services/indexer/src/queue_processor.rs` | Add blob GC timer and handler |

## Configuration Options (Future)

For now, use hardcoded values. Future enhancements could add:
- `BLOB_GC_RETENTION_DAYS` (default: 7)
- `BLOB_GC_BATCH_SIZE` (default: 1000)
- `BLOB_GC_INTERVAL_SECS` (default: 3600)

## Testing Strategy

1. **Unit tests:** Test `gc_orphaned_blobs` with mocked database
2. **Integration tests:**
   - Create documents with content blobs
   - Update documents (creating orphans)
   - Run GC
   - Verify orphaned blobs are deleted, referenced blobs remain

## Edge Cases & Safety

1. **In-flight operations:** 7-day retention ensures blobs from failed operations are cleaned up, but active operations have time to complete
2. **Concurrent access:** GC runs in batches with proper transaction isolation
3. **Foreign key safety:** `ON DELETE SET NULL` ensures deleting a blob won't cascade-delete documents
4. **Deduplication consideration:** SHA256 hash deduplication means multiple documents could share a blob - the LEFT JOIN ensures we only delete truly orphaned blobs

## Rollout Plan

1. Deploy with GC disabled initially (skip the timer tick)
2. Run GC query manually to assess orphan volume
3. Enable GC with conservative batch size
4. Monitor logs for deletion counts and any errors
5. Adjust batch size based on observed performance
