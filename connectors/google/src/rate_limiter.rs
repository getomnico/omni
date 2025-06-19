use anyhow::Result;
use governor::{Quota, RateLimiter as GovernorRateLimiter};
use nonzero_ext::*;
use rand::{thread_rng, Rng};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::warn;

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
}

impl RateLimiter {
    pub fn new(requests_per_second: u32, max_retries: u32) -> Self {
        let requests_per_second =
            std::num::NonZeroU32::new(requests_per_second).unwrap_or(nonzero!(180u32));
        let quota = Quota::per_second(requests_per_second);
        let limiter = Arc::new(GovernorRateLimiter::direct(quota));

        Self {
            limiter,
            max_retries,
        }
    }

    pub async fn check_rate_limit(&self) -> Result<()> {
        self.limiter.until_ready().await;
        Ok(())
    }

    pub async fn execute_with_retry<T, F, Fut>(&self, operation: F) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut retries = 0;
        let mut delay = Duration::from_secs(1);

        loop {
            self.check_rate_limit().await?;

            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    let error_str = e.to_string();

                    if (error_str.contains("403") && error_str.contains("rate limit"))
                        || error_str.contains("429")
                    {
                        if retries >= self.max_retries {
                            return Err(e);
                        }

                        retries += 1;

                        let jitter = thread_rng().gen_range(0..1000);
                        let wait_time = delay + Duration::from_millis(jitter);

                        warn!(
                            "Rate limit hit, retry {} of {}, waiting {:?}",
                            retries, self.max_retries, wait_time
                        );

                        sleep(wait_time).await;

                        delay = delay.saturating_mul(2);
                        if delay > Duration::from_secs(32) {
                            delay = Duration::from_secs(32);
                        }
                    } else {
                        return Err(e);
                    }
                }
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
    async fn test_rate_limiting_basic() {
        let limiter = RateLimiter::new(180, 3);
        // Just test that it doesn't panic and returns Ok
        limiter.check_rate_limit().await.unwrap();
        limiter.check_rate_limit().await.unwrap();
    }

    #[tokio::test]
    async fn test_retry_logic() {
        let limiter = RateLimiter::new(100, 3);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result = limiter
            .execute_with_retry(|| {
                let attempts = Arc::clone(&attempts_clone);
                async move {
                    let count = attempts.fetch_add(1, Ordering::SeqCst);
                    if count < 2 {
                        Err(anyhow!("403: User rate limit exceeded"))
                    } else {
                        Ok("success")
                    }
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }
}
