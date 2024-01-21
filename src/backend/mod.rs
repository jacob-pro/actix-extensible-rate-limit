mod input_builder;

#[cfg(feature = "dashmap")]
#[cfg_attr(docsrs, doc(cfg(feature = "dashmap")))]
pub mod memory;

#[cfg(feature = "redis")]
#[cfg_attr(docsrs, doc(cfg(feature = "redis")))]
pub mod redis;

pub use input_builder::{SimpleInputFunctionBuilder, SimpleInputFuture};
use std::future::Future;

use crate::HeaderCompatibleOutput;
use actix_web::rt::time::Instant;
use std::time::Duration;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Decision {
    Allowed,
    Denied,
}

impl Decision {
    pub fn from_allowed(allowed: bool) -> Self {
        if allowed {
            Self::Allowed
        } else {
            Self::Denied
        }
    }

    pub fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }

    pub fn is_denied(self) -> bool {
        matches!(self, Self::Denied)
    }
}

/// Describes an implementation of a rate limiting store and algorithm.
///
/// A Backend is required to implement [Clone], usually this means wrapping your data store within
/// an [Arc](std::sync::Arc), although many connection pools already do so internally; there is no
/// need to wrap it twice.
pub trait Backend<I: 'static = SimpleInput>: Clone {
    type Output;
    type RollbackToken;
    type Error;

    /// Process an incoming request.
    ///
    /// The input could include such things as a rate limit key, and the rate limit policy to be
    /// applied.
    ///
    /// Returns a boolean of whether to allow or deny the request, arbitrary output that can be used
    /// to transform the allowed and denied responses, and a token to allow the rate limit counter
    /// to be rolled back in certain conditions.
    fn request(
        &self,
        input: I,
    ) -> impl Future<Output = Result<(Decision, Self::Output, Self::RollbackToken), Self::Error>>;

    /// Under certain conditions we may not want to rollback the request operation.
    ///
    /// E.g. We may want to exclude 5xx errors from counting against a user's rate limit,
    /// we can only exclude them after having already allowed the request through the rate limiter
    /// in the first place, so we must therefore deduct from the rate limit counter afterwards.
    ///
    /// Note that if this function fails there is not much the [RateLimiter](crate::RateLimiter)
    /// can do about it, given that the request has already been allowed.
    ///
    /// # Arguments
    ///
    /// * `token`: The token returned from the initial call to [Backend::request()].
    fn rollback(&self, token: Self::RollbackToken)
        -> impl Future<Output = Result<(), Self::Error>>;
}

/// A default [Backend] Input structure.
///
/// This may not be suitable for all use-cases.
#[derive(Debug, Clone)]
pub struct SimpleInput {
    /// The rate limiting interval.
    pub interval: Duration,
    /// The total requests to be allowed within the interval.
    pub max_requests: u64,
    /// The rate limit key to be used for this request.
    pub key: String,
}

/// A default [Backend::Output] structure.
///
/// This may not be suitable for all use-cases.
#[derive(Debug, Clone)]
pub struct SimpleOutput {
    /// Total number of requests that are permitted within the rate limit interval.
    pub limit: u64,
    /// Number of requests that will be permitted until the limit resets.
    pub remaining: u64,
    /// Time at which the rate limit resets.
    pub reset: Instant,
}

/// Additional functions for a [Backend] that uses [SimpleInput] and [SimpleOutput].
pub trait SimpleBackend: Backend<SimpleInput, Output = SimpleOutput> {
    /// Removes the bucket for a given rate limit key.
    ///
    /// Intended to be used to reset a key before changing the interval.
    fn remove_key(&self, key: &str) -> impl Future<Output = Result<(), Self::Error>>;
}

impl HeaderCompatibleOutput for SimpleOutput {
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
        let output = SimpleOutput {
            limit: 0,
            remaining: 0,
            reset: Instant::now() + Duration::from_secs(60),
        };
        tokio::time::advance(Duration::from_secs_f64(29.9)).await;
        // Verify rounded upwards from 30.1
        assert_eq!(output.seconds_until_reset(), 31);
    }
}
