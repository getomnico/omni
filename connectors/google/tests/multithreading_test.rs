use futures::stream::{self, StreamExt};
use shared::RateLimiter;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;

#[tokio::test]
async fn test_concurrent_processing_with_rate_limiting() {
    // Simulate processing 20 files concurrently with rate limiting
    let max_concurrent = 5;
    let rate_limiter = Arc::new(RateLimiter::new(50, 3)); // 50 requests per second
    let semaphore = Arc::new(Semaphore::new(max_concurrent));

    let files = (0..20).collect::<Vec<_>>();
    let start = Instant::now();

    let results = stream::iter(files)
        .map(|file_id| {
            let rate_limiter = Arc::clone(&rate_limiter);
            let semaphore = Arc::clone(&semaphore);

            async move {
                let _permit = semaphore.acquire().await.unwrap();

                // Simulate API call with rate limiting
                rate_limiter
                    .execute_with_retry(|| async {
                        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                        Ok::<_, shared::RetryableError>(format!("processed_file_{}", file_id))
                    })
                    .await
            }
        })
        .buffer_unordered(max_concurrent)
        .collect::<Vec<_>>()
        .await;

    let elapsed = start.elapsed();

    // All files should be processed successfully
    assert_eq!(results.len(), 20);
    for result in results.iter() {
        assert!(result.is_ok());
    }

    // Should complete faster than sequential processing (without being too fast due to rate limiting)
    assert!(
        elapsed.as_secs_f64() < 1.0,
        "Processing took too long: {:?}",
        elapsed
    );

    println!(
        "Processed 20 files in {:?} with {} concurrent workers",
        elapsed, max_concurrent
    );
}

#[tokio::test]
async fn test_rate_limiter_handles_errors() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let rate_limiter = RateLimiter::new(100, 2);
    let attempt_count = AtomicU32::new(0);

    let result = rate_limiter
        .execute_with_retry(|| {
            let count = attempt_count.fetch_add(1, Ordering::SeqCst);
            async move {
                if count < 1 {
                    Err(shared::RetryableError::Transient(anyhow::anyhow!(
                        "403: User rate limit exceeded"
                    )))
                } else {
                    Ok("success")
                }
            }
        })
        .await;

    assert!(result.is_ok());
    assert_eq!(attempt_count.load(Ordering::SeqCst), 2);
}
