mod fixed_window;
#[cfg(feature = "dashmap")]
#[cfg_attr(docsrs, doc(cfg(feature = "dashmap")))]
mod memory;

pub use fixed_window::{FixedWindowBackend, FixedWindowInput, FixedWindowOutput};
#[cfg(feature = "dashmap")]
pub use memory::{FixedWindowInMemory, FixedWindowInMemoryBuilder};

use actix_web::Result;
use async_trait::async_trait;

/// Describes an implementation of a rate limiting store and algorithm.
///
/// To implement your own rate limiting backend it is recommended to use
/// [async_trait](https://github.com/dtolnay/async-trait), and add the `#[async_trait(?Send)]`
/// attribute onto your trait implementation.
///
/// A Backend is required to implement [Clone], usually this means wrapping your data store within
/// an [Arc](std::sync::Arc), although many connection pools already do so internally; there is no
/// need to wrap it twice.
#[async_trait(?Send)]
pub trait Backend<I: 'static>: Clone {
    type Output;
    type RollbackToken;

    /// Process an incoming request.
    ///
    /// The input could include such things as a rate limit key, and the rate limit policy to be
    /// applied.
    ///
    /// Returns a boolean of whether to allow or deny the request, arbitrary output that can be used
    /// to transform the allowed and denied responses, and a token to allow the rate limit counter
    /// to be rolled back in certain conditions.
    async fn request(&self, input: I) -> Result<(bool, Self::Output, Self::RollbackToken)>;

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
    async fn rollback(&self, token: Self::RollbackToken) -> Result<()>;
}
