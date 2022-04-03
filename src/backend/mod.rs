pub mod memory;

use async_trait::async_trait;
use std::time::{Duration, Instant};

#[async_trait]
pub trait Backend: Clone {
    /// Gets and increments the count for a rate limit key
    /// # Arguments
    ///
    /// * `key`: The rate limit key
    /// * `interval`: The interval is required for two reasons: (a) creating a new bucket
    /// and (b) detecting a change in the interval so the count can be reset.
    ///
    /// returns: The current count (after being incremented), and the time the bucket resets.
    async fn get_and_increment(&self, key: &str, interval: Duration) -> (usize, Instant);

    // Under certain conditions we may not want to rollback the increment operation
    // E.g. The service returns a 500 error
    /// # Arguments
    ///
    /// * `key`: The rate limit key
    /// * `interval`: The original interval is required to locate the correct bucket.
    async fn decrement(&self, key: &str, interval: Duration);
}
