use crate::backend::Backend;
use crate::middleware::builder::HeaderCompatibleOutput;
use async_trait::async_trait;
use std::time::{Duration, Instant};

pub struct FixedWindowInput {
    /// The rate limiting interval.
    pub interval: Duration,
    /// The total requests to be allowed within the interval.
    pub max_requests: u64,
    /// The rate limit key to be used for this request.
    pub key: String,
}

pub struct FixedWindowOutput {
    pub limit: u64,
    pub remaining: u64,
    pub reset: Instant,
    /// The rate limit key (allows for a rollback).
    pub key: String,
}

impl HeaderCompatibleOutput for FixedWindowOutput {
    fn limit(&self) -> u64 {
        self.limit
    }

    fn remaining(&self) -> u64 {
        self.remaining
    }

    fn seconds_until_reset(&self) -> u64 {
        self.reset
            .saturating_duration_since(Instant::now())
            .as_secs()
    }
}

#[async_trait(?Send)]
pub trait FixedWindowBackend: Backend<FixedWindowInput, Output = FixedWindowOutput> {
    /// Removes the bucket for a given rate limit key.
    ///
    /// Intended to be used to reset a key before changing the interval.
    async fn remove_key(&self, key: &str) -> Result<(), Box<dyn std::error::Error>>;
}
