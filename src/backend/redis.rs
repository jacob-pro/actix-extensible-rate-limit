use crate::backend::{Backend, SimpleBackend, SimpleInput, SimpleOutput};
use actix_web::rt::time::Instant;
use actix_web::{HttpResponse, ResponseError};
use async_trait::async_trait;
use mobc_redis::mobc::Pool;
use mobc_redis::{mobc, redis, RedisConnectionManager};
use std::borrow::Cow;
use std::ops::DerefMut;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Connection pool error: {0}")]
    ConnectionPool(
        #[source]
        #[from]
        mobc::Error<redis::RedisError>,
    ),
    #[error("Redis error: {0}")]
    Redis(
        #[source]
        #[from]
        redis::RedisError,
    ),
    #[error("Unexpected TTL response")]
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
    pool: Pool<RedisConnectionManager>,
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
    /// # use mobc_redis::{RedisConnectionManager, redis, mobc::Pool};
    /// let client = redis::Client::open("redis://127.0.0.1/").unwrap();
    /// let manager = RedisConnectionManager::new(client);
    /// let pool = Pool::builder().max_open(100).build(manager);
    /// let backend = RedisBackend::builder(pool).build();
    /// ```
    pub fn builder(pool: Pool<RedisConnectionManager>) -> Builder {
        Builder {
            pool,
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
    pool: Pool<RedisConnectionManager>,
    key_prefix: Option<String>,
}

impl Builder {
    /// Apply an optional prefix to all rate limit keys given to this backend.
    ///
    /// This may be useful when the Redis instance is being used for other purposes; the prefix is
    /// used as a 'namespace' to avoid collision with other caches or keys inside Redis.
    pub fn key_prefix(mut self, key_prefix: Option<String>) -> Self {
        self.key_prefix = key_prefix;
        self
    }

    pub fn build(self) -> RedisBackend {
        RedisBackend {
            pool: self.pool,
            key_prefix: self.key_prefix,
        }
    }
}

#[async_trait(?Send)]
impl Backend<SimpleInput> for RedisBackend {
    type Output = SimpleOutput;
    type RollbackToken = String;
    type Error = Error;

    async fn request(
        &self,
        input: SimpleInput,
    ) -> Result<(bool, Self::Output, Self::RollbackToken), Self::Error> {
        let key = self.make_key(&input.key);
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("SET") // Set key and value
            .arg(key.as_ref())
            .arg(0u64)
            .arg("EX") // Set the specified expire time, in seconds.
            .arg(input.interval.as_secs())
            .arg("NX") // Only set the key if it does not already exist.
            .ignore() // --- ignore returned value of SET command ---
            .cmd("INCR") // Increment key
            .arg(key.as_ref())
            .cmd("TTL") // Return time-to-live of key
            .arg(key.as_ref());

        let mut connection = self.pool.get().await?;
        let (count, ttl): (u64, i64) = pipe.query_async(connection.deref_mut()).await?;
        if ttl < 0 {
            return Err(Self::Error::NegativeTtl);
        }

        let allow = count <= input.max_requests;
        let output = SimpleOutput {
            limit: input.max_requests,
            remaining: input.max_requests.saturating_sub(count),
            reset: Instant::now() + Duration::from_secs(ttl as u64),
        };
        Ok((allow, output, input.key))
    }

    async fn rollback(&self, _token: Self::RollbackToken) -> Result<(), Self::Error> {
        unimplemented!();
    }
}

#[async_trait(?Send)]
impl SimpleBackend for RedisBackend {
    async fn remove_key(&self, key: &str) -> Result<(), Self::Error> {
        let key = self.make_key(key);
        let mut connection = self.pool.get().await?;
        redis::cmd("DEL")
            .arg(key.as_ref())
            .query_async::<_, ()>(connection.deref_mut())
            .await?;
        Ok(())
    }
}
