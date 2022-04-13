use crate::backend::Backend;
use crate::middleware::builder::HeaderCompatibleOutput;
use actix_web::rt::time::Instant;
use async_trait::async_trait;
use std::time::Duration;

mod input_builder;
#[cfg(feature = "dashmap")]
#[cfg_attr(docsrs, doc(cfg(feature = "dashmap")))]
mod memory;

pub use input_builder::SimpleInputFunctionBuilder;
#[cfg(feature = "dashmap")]
pub use memory::{InMemoryBackend, InMemoryBackendBuilder};

#[async_trait(?Send)]
pub trait FixedWindowBackend: Backend<FixedWindowInput, Output = FixedWindowOutput> {
    /// Removes the bucket for a given rate limit key.
    ///
    /// Intended to be used to reset a key before changing the interval.
    async fn remove_key(&self, key: &str) -> Result<(), Box<dyn std::error::Error>>;
}

/// Input for a [FixedWindowBackend].
#[derive(Debug, Clone)]
pub struct FixedWindowInput {
    /// The rate limiting interval.
    pub interval: Duration,
    /// The total requests to be allowed within the interval.
    pub max_requests: u64,
    /// The rate limit key to be used for this request.
    pub key: String,
}

/// Output from a [FixedWindowBackend].
#[derive(Debug, Clone)]
pub struct FixedWindowOutput {
    /// Total number of requests that are permitted within the rate limit interval.
    pub limit: u64,
    /// Number of requests that will be permitted until the limit resets.
    pub remaining: u64,
    /// Time at which the rate limit resets.
    pub reset: Instant,
}

impl HeaderCompatibleOutput for FixedWindowOutput {
    fn limit(&self) -> u64 {
        self.limit
    }

    fn remaining(&self) -> u64 {
        self.remaining
    }

    /// Seconds until the rate limit resets (rounded upwards, so that it is guaranteed to be reset
    /// after waiting for the duration).
    fn seconds_until_reset(&self) -> u64 {
        let millis = self
            .reset
            .saturating_duration_since(Instant::now())
            .as_millis() as f64;
        (millis / 1000f64).ceil() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[actix_web::test]
    async fn test_seconds_until_reset() {
        tokio::time::pause();
        let output = FixedWindowOutput {
            limit: 0,
            remaining: 0,
            reset: Instant::now() + Duration::from_secs(60),
        };
        tokio::time::advance(Duration::from_secs_f64(29.9)).await;
        // Verify rounded upwards from 30.1
        assert_eq!(output.seconds_until_reset(), 31);
    }
}
