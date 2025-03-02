use crate::backend::{Backend, Decision, SimpleBackend, SimpleInput, SimpleOutput};
use actix_web::rt::time::Instant;
use actix_web::{HttpResponse, ResponseError};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::borrow::Cow;
use std::time::Duration;
use thiserror::Error;

const BITFIELD_ENCODING: &str = "u63";
const BITFIELD_OFFSET: u8 = 0;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Redis error: {0}")]
    Redis(
        #[source]
        #[from]
        redis::RedisError,
    ),
    #[error("Unexpected negative TTL response for the rate limit key")]
    NegativeTtl,
}

impl ResponseError for Error {
    fn error_response(&self) -> HttpResponse {
        HttpResponse::InternalServerError().finish()
    }
}

/// A Fixed Window rate limiter [Backend] that uses stores data in Redis.
#[derive(Clone)]
pub struct RedisBackend {
    connection: ConnectionManager,
    key_prefix: Option<String>,
}

impl RedisBackend {
    /// Create a RedisBackendBuilder.
    ///
    /// # Arguments
    ///
    /// * `pool`: [A Redis connection pool](https://github.com/importcjj/mobc-redis)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use actix_extensible_rate_limit::backend::redis::RedisBackend;
    /// # use redis::aio::ConnectionManager;
    /// # async fn example() {
    /// let client = redis::Client::open("redis://127.0.0.1/").unwrap();
    /// let manager = ConnectionManager::new(client).await.unwrap();
    /// let backend = RedisBackend::builder(manager).build();
    /// # };
    /// ```
    pub fn builder(connection: ConnectionManager) -> Builder {
        Builder {
            connection,
            key_prefix: None,
        }
    }

    fn make_key<'t>(&self, key: &'t str) -> Cow<'t, str> {
        match &self.key_prefix {
            None => Cow::Borrowed(key),
            Some(prefix) => Cow::Owned(format!("{prefix}{key}")),
        }
    }
}

pub struct Builder {
    connection: ConnectionManager,
    key_prefix: Option<String>,
}

impl Builder {
    /// Apply an optional prefix to all rate limit keys given to this backend.
    ///
    /// This may be useful when the Redis instance is being used for other purposes; the prefix is
    /// used as a 'namespace' to avoid collision with other caches or keys inside Redis.
    pub fn key_prefix(mut self, key_prefix: Option<&str>) -> Self {
        self.key_prefix = key_prefix.map(ToOwned::to_owned);
        self
    }

    pub fn build(self) -> RedisBackend {
        RedisBackend {
            connection: self.connection,
            key_prefix: self.key_prefix,
        }
    }
}

impl Backend<SimpleInput> for RedisBackend {
    type Output = SimpleOutput;
    type RollbackToken = String;
    type Error = Error;

    async fn request(
        &self,
        input: SimpleInput,
    ) -> Result<(Decision, Self::Output, Self::RollbackToken), Self::Error> {
        let key = self.make_key(&input.key);

        let mut pipe = redis::pipe();
        pipe.atomic()
            // Increment the rate limit count
            .cmd("BITFIELD")
            .arg(key.as_ref())
            .arg("OVERFLOW")
            .arg("SAT")
            .arg("INCRBY")
            .arg(BITFIELD_ENCODING)
            .arg(BITFIELD_OFFSET)
            .arg(1)
            .arg("GET")
            .arg(BITFIELD_ENCODING)
            .arg(BITFIELD_OFFSET)
            // Set the key to expire (only if it doesn't already have an expiry)
            .cmd("EXPIRE")
            .arg(key.as_ref())
            .arg(input.interval.as_secs())
            .arg("NX")
            .ignore()
            // Return time-to-live of key
            .cmd("TTL")
            .arg(key.as_ref());

        let mut con = self.connection.clone();
        let (counts, ttl): (Vec<u64>, i64) = pipe.query_async(&mut con).await?;
        if ttl < 0 {
            return Err(Error::NegativeTtl);
        }
        let count = *counts.first().expect("BITFIELD should return one value");

        let allow = count <= input.max_requests;
        let output = SimpleOutput {
            limit: input.max_requests,
            remaining: input.max_requests.saturating_sub(count),
            reset: Instant::now() + Duration::from_secs(ttl as u64),
        };
        Ok((Decision::from_allowed(allow), output, input.key))
    }

    async fn rollback(&self, token: Self::RollbackToken) -> Result<(), Self::Error> {
        let key = self.make_key(&token);

        let mut con = self.connection.clone();

        let mut pipe = redis::pipe();
        pipe.atomic()
            // Decrement the rate limit count
            .cmd("BITFIELD")
            .arg(key.as_ref())
            .arg("OVERFLOW")
            .arg("SAT")
            .arg("INCRBY")
            .arg(BITFIELD_ENCODING)
            .arg(BITFIELD_OFFSET)
            .arg(-1)
            // Set the key to expire immediately, if it doesn't already have an expiry
            .cmd("EXPIRE")
            .arg(key.as_ref())
            .arg(0)
            .arg("NX")
            .ignore();

        let () = pipe.query_async(&mut con).await?;

        Ok(())
    }
}

impl SimpleBackend for RedisBackend {
    /// Note that the key prefix (if set) is automatically included, you do not need to prepend
    /// it yourself.
    async fn remove_key(&self, key: &str) -> Result<(), Self::Error> {
        let key = self.make_key(key);
        let mut con = self.connection.clone();
        let () = con.del(key.as_ref()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HeaderCompatibleOutput;
    use redis::Cmd;

    const MINUTE: Duration = Duration::from_secs(60);

    // Each test must use non-overlapping keys (because the tests may be run concurrently)
    // Each test should also reset its key on each run, so that it is in a clean state.
    async fn make_backend(clear_test_key: &str) -> Builder {
        let host = option_env!("REDIS_HOST").unwrap_or("127.0.0.1");
        let port = option_env!("REDIS_PORT").unwrap_or("6379");
        let client = redis::Client::open(format!("redis://{host}:{port}")).unwrap();
        let mut manager = ConnectionManager::new(client).await.unwrap();
        manager.del::<_, ()>(clear_test_key).await.unwrap();
        RedisBackend::builder(manager)
    }

    #[actix_web::test]
    async fn test_allow_deny() {
        let backend = make_backend("test_allow_deny").await.build();
        let input = SimpleInput {
            interval: MINUTE,
            max_requests: 5,
            key: "test_allow_deny".to_string(),
        };
        let mut prev_seconds_until_reset = u64::MAX;
        for i in (0..5).rev() {
            // First 5 should be allowed
            let (decision, output, _) = backend.request(input.clone()).await.unwrap();
            // Remaining counts should be decreasing
            assert_eq!(output.remaining, i);
            // Limit should be the same
            assert_eq!(output.limit, 5);
            // Request should be allowed
            assert!(decision.is_allowed());
            // Check expiry time is going down each time (instead of being reset)
            assert!(output.seconds_until_reset() < prev_seconds_until_reset);
            // Sleep for a second
            prev_seconds_until_reset = output.seconds_until_reset();
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        // Sixth should be denied
        let (decision, output, _) = backend.request(input.clone()).await.unwrap();
        assert_eq!(output.remaining, 0);
        assert_eq!(output.limit, 5);
        assert!(decision.is_denied());
    }

    #[actix_web::test]
    async fn test_reset() {
        let backend = make_backend("test_reset").await.build();
        let input = SimpleInput {
            interval: Duration::from_secs(3),
            max_requests: 1,
            key: "test_reset".to_string(),
        };
        // Make first request, should be allowed
        let (decision, _, _) = backend.request(input.clone()).await.unwrap();
        assert!(decision.is_allowed());

        // Request again immediately afterwards, should now be denied
        let (decision, out, _) = backend.request(input.clone()).await.unwrap();
        assert!(decision.is_denied());

        // Sleep until reset, should now be allowed
        tokio::time::sleep(Duration::from_secs(out.seconds_until_reset())).await;
        let (decision, _, _) = backend.request(input).await.unwrap();
        assert!(decision.is_allowed());
    }

    #[actix_web::test]
    async fn test_output() {
        let backend = make_backend("test_output").await.build();
        let input = SimpleInput {
            interval: MINUTE,
            max_requests: 2,
            key: "test_output".to_string(),
        };
        // First of 2 should be allowed.
        let (decision, output, _) = backend.request(input.clone()).await.unwrap();
        assert!(decision.is_allowed());
        assert_eq!(output.remaining, 1);
        assert_eq!(output.limit, 2);
        assert!(output.seconds_until_reset() > 0 && output.seconds_until_reset() <= 60);

        // Second of 2 should be allowed.
        let (decision, output, _) = backend.request(input.clone()).await.unwrap();
        assert!(decision.is_allowed());
        assert_eq!(output.remaining, 0);
        assert_eq!(output.limit, 2);
        assert!(output.seconds_until_reset() > 0 && output.seconds_until_reset() <= 60);

        // Should be denied
        let (decision, output, _) = backend.request(input).await.unwrap();
        assert!(decision.is_denied());
        assert_eq!(output.remaining, 0);
        assert_eq!(output.limit, 2);
        assert!(output.seconds_until_reset() > 0 && output.seconds_until_reset() <= 60);
    }

    #[actix_web::test]
    async fn test_rollback() {
        let backend = make_backend("test_rollback").await.build();
        let input = SimpleInput {
            interval: MINUTE,
            max_requests: 5,
            key: "test_rollback".to_string(),
        };
        let (_, output, rollback) = backend.request(input.clone()).await.unwrap();
        assert_eq!(output.remaining, 4);
        backend.rollback(rollback).await.unwrap();
        // Remaining requests should still be the same, since the previous call was excluded
        let (_, output, _) = backend.request(input).await.unwrap();
        assert_eq!(output.remaining, 4);
        // Check ttl is not corrupted
        assert!(output.seconds_until_reset() > 0 && output.seconds_until_reset() <= 60);
    }

    #[actix_web::test]
    async fn test_rollback_key_gone() {
        let key = "test_rollback_key_gone";
        let backend = make_backend(key).await.build();
        let mut con = backend.connection.clone();
        // The rollback could happen after the key has already expired / gone
        backend.rollback(key.to_string()).await.unwrap();
        // In which case the count should remain at 0 (it must not become negative)
        let mut cmd = Cmd::new();
        cmd.arg("BITFIELD")
            .arg(key)
            .arg("GET")
            .arg(BITFIELD_ENCODING)
            .arg(BITFIELD_OFFSET);
        let value: Vec<u64> = cmd.query_async(&mut con).await.unwrap();
        assert_eq!(value[0], 0u64);
    }

    #[actix_web::test]
    async fn test_remove_key() {
        let backend = make_backend("test_remove_key").await.build();
        let input = SimpleInput {
            interval: MINUTE,
            max_requests: 1,
            key: "test_remove_key".to_string(),
        };
        let (decision, _, _) = backend.request(input.clone()).await.unwrap();
        assert!(decision.is_allowed());
        let (decision, _, _) = backend.request(input.clone()).await.unwrap();
        assert!(decision.is_denied());
        backend.remove_key("test_remove_key").await.unwrap();
        // Counter should have been reset
        let (decision, _, _) = backend.request(input).await.unwrap();
        assert!(decision.is_allowed());
    }

    #[actix_web::test]
    async fn test_key_prefix() {
        let backend = make_backend("prefix:test_key_prefix")
            .await
            .key_prefix(Some("prefix:"))
            .build();
        let mut con = backend.connection.clone();
        let input = SimpleInput {
            interval: MINUTE,
            max_requests: 5,
            key: "test_key_prefix".to_string(),
        };
        backend.request(input.clone()).await.unwrap();
        assert!(con
            .exists::<_, bool>("prefix:test_key_prefix")
            .await
            .unwrap());

        backend.remove_key("test_key_prefix").await.unwrap();
        assert!(!con
            .exists::<_, bool>("prefix:test_key_prefix")
            .await
            .unwrap());
    }
}
