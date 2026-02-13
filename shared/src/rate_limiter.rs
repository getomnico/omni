use anyhow::Result;
use governor::{Quota, RateLimiter as GovernorRateLimiter};
use rand::{thread_rng, Rng};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::time::sleep;
use tracing::{debug, info, warn};

/// Error type for operations executed via `execute_with_retry`.
#[derive(Debug)]
pub enum RetryableError {
    /// Upstream rate limit (e.g. HTTP 429) with a server-specified wait duration.
    RateLimited {
        retry_after: Duration,
        message: String,
    },
    /// Transient error — retried with exponential backoff.
    Transient(anyhow::Error),
    /// Permanent error — not retried.
    Permanent(anyhow::Error),
}

impl std::fmt::Display for RetryableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited {
                retry_after,
                message,
            } => write!(
                f,
                "Rate limited: {} (retry after {:?})",
                message, retry_after
            ),
            Self::Transient(e) => write!(f, "{}", e),
            Self::Permanent(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for RetryableError {}

impl From<anyhow::Error> for RetryableError {
    fn from(e: anyhow::Error) -> Self {
        Self::Transient(e)
    }
}

#[derive(Clone)]
pub struct RateLimiter {
    limiter: Arc<
        GovernorRateLimiter<
            governor::state::direct::NotKeyed,
            governor::state::InMemoryState,
            governor::clock::DefaultClock,
        >,
    >,
    max_retries: u32,
    request_count: Arc<AtomicU64>,
    last_log_time: Arc<std::sync::Mutex<Instant>>,
    configured_rps: u32,
}

impl RateLimiter {
    const MAX_BACKOFF: Duration = Duration::from_secs(32); // Maximum backoff time in seconds

    pub fn new(requests_per_second: u32, max_retries: u32) -> Self {
        let requests_per_second_nz = NonZeroU32::new(requests_per_second).expect(
            format!(
                "Invalid requests_per_second for RateLimiter: {}",
                requests_per_second
            )
            .as_str(),
        );
        let quota = Quota::per_second(requests_per_second_nz);
        let limiter = Arc::new(GovernorRateLimiter::direct(quota));

        debug!(
            "Creating rate limit with limit of {} requests per second",
            requests_per_second
        );
        Self {
            limiter,
            max_retries,
            request_count: Arc::new(AtomicU64::new(0)),
            last_log_time: Arc::new(std::sync::Mutex::new(Instant::now())),
            configured_rps: requests_per_second,
        }
    }

    pub async fn check_rate_limit(&self) -> Result<()> {
        self.limiter.until_ready().await;

        self.request_count.fetch_add(1, Ordering::Relaxed);

        let mut last_log = self.last_log_time.lock().unwrap();
        let elapsed = last_log.elapsed();

        if elapsed >= Duration::from_secs(1) {
            let count = self.request_count.swap(0, Ordering::Relaxed);
            let actual_rps = count as f64 / elapsed.as_secs_f64();

            info!(
                "Rate limiter stats: actual={:.2} req/sec, limit={} req/sec",
                actual_rps, self.configured_rps
            );

            *last_log = Instant::now();
        }

        Ok(())
    }

    pub async fn execute_with_retry<T, F, Fut>(&self, operation: F) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = std::result::Result<T, RetryableError>>,
    {
        let mut retries = 0;
        let mut delay = Duration::from_secs(1);

        loop {
            self.check_rate_limit().await?;

            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) => match e {
                    RetryableError::Permanent(e) => return Err(e),
                    RetryableError::RateLimited {
                        retry_after,
                        ref message,
                    } => {
                        if retries >= self.max_retries {
                            return Err(anyhow::anyhow!("{}", e));
                        }
                        retries += 1;
                        warn!(
                            "Rate limited: {}, retry {} of {}, waiting {:?}",
                            message, retries, self.max_retries, retry_after
                        );
                        sleep(retry_after).await;
                    }
                    RetryableError::Transient(e) => {
                        if retries >= self.max_retries {
                            return Err(e);
                        }
                        retries += 1;
                        let jitter = thread_rng().gen_range(0..1000);
                        let wait_time = delay + Duration::from_millis(jitter);
                        warn!(
                            "Transient error: {}, retry {} of {}, waiting {:?}",
                            e, retries, self.max_retries, wait_time
                        );
                        sleep(wait_time).await;
                        delay = delay.saturating_mul(2);
                        if delay > Self::MAX_BACKOFF {
                            delay = Self::MAX_BACKOFF;
                        }
                    }
                },
            }
        }
    }

    pub async fn execute<T, F, Fut>(&self, operation: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        self.check_rate_limit().await?;
        operation().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_execute_success() {
        let limiter = RateLimiter::new(100, 3);

        let result = limiter
            .execute(|| async { Ok::<_, anyhow::Error>(42) })
            .await;

        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_execute_propagates_error() {
        let limiter = RateLimiter::new(100, 3);

        let result = limiter
            .execute(|| async { Err::<i32, _>(anyhow!("operation failed")) })
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("operation failed"));
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_transient_failures() {
        let limiter = RateLimiter::new(100, 3);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result = limiter
            .execute_with_retry(|| {
                let attempts = Arc::clone(&attempts_clone);
                async move {
                    let count = attempts.fetch_add(1, Ordering::SeqCst);
                    if count < 2 {
                        Err(RetryableError::Transient(anyhow!(
                            "403: User rate limit exceeded"
                        )))
                    } else {
                        Ok("success")
                    }
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhaustion_returns_last_error() {
        let limiter = RateLimiter::new(100, 2);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result = limiter
            .execute_with_retry(|| {
                let attempts = Arc::clone(&attempts_clone);
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(RetryableError::Transient(anyhow!("persistent failure")))
                }
            })
            .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("persistent failure"));
        // 1 initial attempt + 2 retries = 3 total attempts
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_permanent_error_not_retried() {
        let limiter = RateLimiter::new(100, 3);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result = limiter
            .execute_with_retry(|| {
                let attempts = Arc::clone(&attempts_clone);
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(RetryableError::Permanent(anyhow!("404 not found")))
                }
            })
            .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_rate_limited_uses_retry_after() {
        let limiter = RateLimiter::new(100, 3);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let start = Instant::now();
        let result = limiter
            .execute_with_retry(|| {
                let attempts = Arc::clone(&attempts_clone);
                async move {
                    let count = attempts.fetch_add(1, Ordering::SeqCst);
                    if count == 0 {
                        Err(RetryableError::RateLimited {
                            retry_after: Duration::from_millis(100),
                            message: "429 too many requests".to_string(),
                        })
                    } else {
                        Ok("success")
                    }
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert!(start.elapsed() >= Duration::from_millis(100));
    }
}
