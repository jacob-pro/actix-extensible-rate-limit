use crate::backend::{Backend, Decision, SimpleBackend, SimpleInput, SimpleOutput};
use actix_web::rt::task::JoinHandle;
use actix_web::rt::time::Instant;
use dashmap::DashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_GC_INTERVAL_SECONDS: u64 = 60 * 10;

/// A Fixed Window rate limiter [Backend] that uses [Dashmap](dashmap::DashMap) to store keys
/// in memory.
#[derive(Clone)]
pub struct InMemoryBackend {
    map: Arc<DashMap<String, Value>>,
    gc_handle: Option<Arc<JoinHandle<()>>>,
}

struct Value {
    ttl: Instant,
    count: u64,
}

impl InMemoryBackend {
    pub fn builder() -> Builder {
        Builder {
            gc_interval: Some(Duration::from_secs(DEFAULT_GC_INTERVAL_SECONDS)),
        }
    }

    fn garbage_collector(map: Arc<DashMap<String, Value>>, interval: Duration) -> JoinHandle<()> {
        assert!(
            interval.as_secs_f64() > 0f64,
            "GC interval must be non-zero"
        );
        actix_web::rt::spawn(async move {
            loop {
                let now = Instant::now();
                map.retain(|_k, v| v.ttl > now);
                actix_web::rt::time::sleep_until(now + interval).await;
            }
        })
    }
}

pub struct Builder {
    gc_interval: Option<Duration>,
}

impl Builder {
    /// Override the default garbage collector interval.
    ///
    /// Set to None to disable garbage collection.
    ///
    /// The garbage collector periodically scans the internal map, removing expired buckets.
    pub fn with_gc_interval(mut self, interval: Option<Duration>) -> Self {
        self.gc_interval = interval;
        self
    }

    pub fn build(self) -> InMemoryBackend {
        let map = Arc::new(DashMap::<String, Value>::new());
        let gc_handle = self.gc_interval.map(|gc_interval| {
            Arc::new(InMemoryBackend::garbage_collector(map.clone(), gc_interval))
        });
        InMemoryBackend { map, gc_handle }
    }
}

impl Backend<SimpleInput> for InMemoryBackend {
    type Output = SimpleOutput;
    type RollbackToken = String;
    type Error = Infallible;

    async fn request(
        &self,
        input: SimpleInput,
    ) -> Result<(Decision, Self::Output, Self::RollbackToken), Self::Error> {
        let now = Instant::now();
        let mut count = 1;
        let mut expiry = now
            .checked_add(input.interval)
            .expect("Interval unexpectedly large");
        self.map
            .entry(input.key.clone())
            .and_modify(|v| {
                // If this bucket hasn't yet expired, increment and extract the count/expiry
                if v.ttl > now {
                    v.count += 1;
                    count = v.count;
                    expiry = v.ttl;
                } else {
                    // If this bucket has expired we will reset the count to 1 and set a new TTL.
                    v.ttl = expiry;
                    v.count = count;
                }
            })
            .or_insert_with(|| Value {
                // If the bucket doesn't exist, create it with a count of 1, and set the TTL.
                ttl: expiry,
                count,
            });
        let allow = count <= input.max_requests;
        let output = SimpleOutput {
            limit: input.max_requests,
            remaining: input.max_requests.saturating_sub(count),
            reset: expiry,
        };
        Ok((Decision::from_allowed(allow), output, input.key))
    }

    async fn rollback(&self, token: Self::RollbackToken) -> Result<(), Self::Error> {
        self.map.entry(token).and_modify(|v| {
            v.count = v.count.saturating_sub(1);
        });
        Ok(())
    }
}

impl SimpleBackend for InMemoryBackend {
    async fn remove_key(&self, key: &str) -> Result<(), Self::Error> {
        self.map.remove(key);
        Ok(())
    }
}

impl Drop for InMemoryBackend {
    fn drop(&mut self) {
        if let Some(handle) = &self.gc_handle {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    #[actix_web::test]
    async fn test_allow_deny() {
        tokio::time::pause();
        let backend = InMemoryBackend::builder().build();
        let input = SimpleInput {
            interval: MINUTE,
            max_requests: 5,
            key: "KEY1".to_string(),
        };
        for _ in 0..5 {
            // First 5 should be allowed
            let (allow, _, _) = backend.request(input.clone()).await.unwrap();
            assert!(allow.is_allowed());
        }
        // Sixth should be denied
        let (allow, _, _) = backend.request(input.clone()).await.unwrap();
        assert!(!allow.is_allowed());
    }

    #[actix_web::test]
    async fn test_reset() {
        tokio::time::pause();
        let backend = InMemoryBackend::builder().with_gc_interval(None).build();
        let input = SimpleInput {
            interval: MINUTE,
            max_requests: 1,
            key: "KEY1".to_string(),
        };
        // Make first request, should be allowed
        let (decision, _, _) = backend.request(input.clone()).await.unwrap();
        assert!(decision.is_allowed());
        // Request again, should be denied
        let (decision, _, _) = backend.request(input.clone()).await.unwrap();
        assert!(decision.is_denied());
        // Advance time and try again, should now be allowed
        tokio::time::advance(MINUTE).await;
        // We want to be sure the key hasn't been garbage collected, and we are testing the expiry logic
        assert!(backend.map.contains_key("KEY1"));
        let (decision, _, _) = backend.request(input).await.unwrap();
        assert!(decision.is_allowed());
    }

    #[actix_web::test]
    async fn test_garbage_collection() {
        tokio::time::pause();
        let backend = InMemoryBackend::builder()
            .with_gc_interval(Some(MINUTE))
            .build();
        backend
            .request(SimpleInput {
                interval: MINUTE,
                max_requests: 1,
                key: "KEY1".to_string(),
            })
            .await
            .unwrap();
        backend
            .request(SimpleInput {
                interval: MINUTE * 2,
                max_requests: 1,
                key: "KEY2".to_string(),
            })
            .await
            .unwrap();
        assert!(backend.map.contains_key("KEY1"));
        assert!(backend.map.contains_key("KEY2"));
        // Advance time such that the garbage collector runs,
        // expired KEY1 should be cleaned, but KEY2 should remain.
        tokio::time::advance(MINUTE).await;
        assert!(!backend.map.contains_key("KEY1"));
        assert!(backend.map.contains_key("KEY2"));
    }

    #[actix_web::test]
    async fn test_output() {
        tokio::time::pause();
        let backend = InMemoryBackend::builder().build();
        let input = SimpleInput {
            interval: MINUTE,
            max_requests: 2,
            key: "KEY1".to_string(),
        };
        // First of 2 should be allowed.
        let (decision, output, _) = backend.request(input.clone()).await.unwrap();
        assert!(decision.is_allowed());
        assert_eq!(output.remaining, 1);
        assert_eq!(output.limit, 2);
        assert_eq!(output.reset, Instant::now() + MINUTE);
        // Second of 2 should be allowed.
        let (decision, output, _) = backend.request(input.clone()).await.unwrap();
        assert!(decision.is_allowed());
        assert_eq!(output.remaining, 0);
        assert_eq!(output.limit, 2);
        assert_eq!(output.reset, Instant::now() + MINUTE);
        // Should be denied
        let (decision, output, _) = backend.request(input).await.unwrap();
        assert!(decision.is_denied());
        assert_eq!(output.remaining, 0);
        assert_eq!(output.limit, 2);
        assert_eq!(output.reset, Instant::now() + MINUTE);
    }

    #[actix_web::test]
    async fn test_rollback() {
        tokio::time::pause();
        let backend = InMemoryBackend::builder().build();
        let input = SimpleInput {
            interval: MINUTE,
            max_requests: 5,
            key: "KEY1".to_string(),
        };
        let (_, output, rollback) = backend.request(input.clone()).await.unwrap();
        assert_eq!(output.remaining, 4);
        backend.rollback(rollback).await.unwrap();
        // Remaining requests should still be the same, since the previous call was excluded
        let (_, output, _) = backend.request(input).await.unwrap();
        assert_eq!(output.remaining, 4);
    }

    #[actix_web::test]
    async fn test_remove_key() {
        tokio::time::pause();
        let backend = InMemoryBackend::builder().with_gc_interval(None).build();
        let input = SimpleInput {
            interval: MINUTE,
            max_requests: 1,
            key: "KEY1".to_string(),
        };
        let (decision, _, _) = backend.request(input.clone()).await.unwrap();
        assert!(decision.is_allowed());
        let (decision, _, _) = backend.request(input.clone()).await.unwrap();
        assert!(decision.is_denied());
        backend.remove_key("KEY1").await.unwrap();
        // Counter should have been reset
        let (decision, _, _) = backend.request(input).await.unwrap();
        assert!(decision.is_allowed());
    }
}
