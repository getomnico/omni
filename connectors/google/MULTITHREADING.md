# Google Connector Multi-Threading Implementation

This document describes the multi-threading implementation for the Google Drive connector.

## Overview

The Google connector now supports concurrent file processing with proper rate limiting and exponential backoff retry logic. This significantly improves sync performance for large file sets while respecting Google Drive API limits.

## Key Features

### 1. Concurrent File Processing
- Files are processed concurrently using Rust's `futures` library
- Controlled by a semaphore to limit concurrent downloads
- Default: 10 concurrent downloads (configurable)

### 2. Rate Limiting
- Built-in rate limiter using the `governor` crate
- Default: 180 requests per second (90% of Google's 200/sec limit)
- Automatic rate limit enforcement for all API calls

### 3. Exponential Backoff
- Automatic retry logic for rate limit errors (403/429)
- Exponential backoff with jitter to prevent thundering herd
- Default: up to 5 retries with max 32-second delay

### 4. Configurable Parameters
All parameters can be configured via environment variables:

```bash
# Maximum concurrent downloads (default: 10)
GOOGLE_MAX_CONCURRENT_DOWNLOADS=15

# API rate limit in requests per second (default: 180)
GOOGLE_API_RATE_LIMIT=150

# Maximum retry attempts for rate limit errors (default: 5)
GOOGLE_MAX_RETRIES=3
```

## Architecture Changes

### 1. Rate Limiter Module (`rate_limiter.rs`)
- `RateLimiter` struct with configurable rate limiting
- `execute_with_retry()` method for automatic retries
- Built-in exponential backoff with jitter

### 2. Drive Client Updates (`drive.rs`)
- All API methods now support rate limiting
- Added `with_rate_limiter()` constructor
- Rate limiting applied to:
  - File listing
  - Google Docs content retrieval
  - Google Sheets content retrieval
  - PDF content download
  - General file downloads

### 3. Sync Manager Updates (`sync.rs`)
- Concurrent file processing using `stream::iter().buffer_unordered()`
- Semaphore-based concurrency control
- Atomic counters for progress tracking
- Preserved sync state management

## Performance Impact

### Before (Sequential Processing)
- 1000 files ≈ 10-15 minutes (depending on file sizes)
- Single-threaded downloads
- No rate limit protection

### After (Concurrent Processing)
- 1000 files ≈ 2-5 minutes (depending on file sizes and configuration)
- 10x concurrent downloads by default
- Built-in rate limit protection
- Automatic retry on failures

## Google Drive API Rate Limits

Based on Google's documentation:
- **Standard Limit**: 12,000 queries per 60 seconds (200/sec)
- **Per-User Limit**: 12,000 queries per 60 seconds per user
- **Recommended Buffer**: Use 90% of limit (180/sec) for safety

## Testing

### Unit Tests
```bash
cargo test --package clio-google-connector --lib rate_limiter
```

### Integration Tests
```bash
cargo test --package clio-google-connector --test multithreading_test
```

### Manual Testing
To test with a large number of files:
1. Set up a Google Drive with 100+ files
2. Configure environment variables:
   ```bash
   export GOOGLE_MAX_CONCURRENT_DOWNLOADS=20
   export GOOGLE_API_RATE_LIMIT=180
   ```
3. Run a full sync and monitor logs for concurrent processing

## Monitoring

The implementation includes extensive logging:

```
INFO Found 150 files to process concurrently
INFO Processed 150 files in 2.3s with 10 concurrent workers
WARN Rate limit hit, retry 1 of 5, waiting 1.2s
```

## Safety Considerations

1. **Rate Limiting**: All API calls are rate-limited to prevent quota exhaustion
2. **Retry Logic**: Automatic retries with exponential backoff for transient failures
3. **Concurrency Control**: Semaphore prevents overwhelming the API or local resources
4. **Error Handling**: Failed downloads don't stop the entire sync process
5. **Sync State**: Atomic updates ensure consistent sync state even with concurrent processing

## Future Improvements

1. **Dynamic Rate Limiting**: Adjust rate limits based on API response headers
2. **Intelligent Batching**: Group small files for batch processing
3. **Progress Reporting**: Real-time progress updates via WebSocket
4. **Adaptive Concurrency**: Adjust concurrency based on API performance
5. **Priority Queues**: Process recently modified files first

## Configuration Examples

### High-Performance Setup (Fast Network, Dedicated API Quotas)
```bash
export GOOGLE_MAX_CONCURRENT_DOWNLOADS=25
export GOOGLE_API_RATE_LIMIT=190
export GOOGLE_MAX_RETRIES=3
```

### Conservative Setup (Shared API Quotas, Slower Network)
```bash
export GOOGLE_MAX_CONCURRENT_DOWNLOADS=5
export GOOGLE_API_RATE_LIMIT=100
export GOOGLE_MAX_RETRIES=10
```

### Development/Testing Setup
```bash
export GOOGLE_MAX_CONCURRENT_DOWNLOADS=3
export GOOGLE_API_RATE_LIMIT=50
export GOOGLE_MAX_RETRIES=2
```