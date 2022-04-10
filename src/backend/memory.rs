use crate::backend::fixed_window::{FixedWindowBackend, FixedWindowInput, FixedWindowOutput};
use crate::backend::Backend;
use actix_web::rt::task::JoinHandle;
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const DEFAULT_GC_INTERVAL_SECONDS: u64 = 60 * 10;

/// A Fixed Window rate limiter [Backend] that uses [Dashmap](dashmap::DashMap) to store keys
/// in memory.
#[derive(Clone)]
pub struct FixedWindowInMemory {
    map: Arc<DashMap<String, Value>>,
    gc_handle: Arc<JoinHandle<()>>,
}

struct Value {
    ttl: Instant,
    count: u64,
}

impl FixedWindowInMemory {
    pub fn builder() -> FixedWindowInMemoryBuilder {
        FixedWindowInMemoryBuilder {
            gc_interval: Duration::from_secs(DEFAULT_GC_INTERVAL_SECONDS),
        }
    }

    fn garbage_collector(map: Arc<DashMap<String, Value>>, interval: Duration) -> JoinHandle<()> {
        actix_web::rt::spawn(async move {
            loop {
                let now = Instant::now();
                map.retain(|_k, v| v.ttl > now);
                actix_web::rt::time::sleep_until((now + interval).into()).await;
            }
        })
    }
}

#[async_trait(?Send)]
impl Backend<FixedWindowInput> for FixedWindowInMemory {
    type Output = FixedWindowOutput;
    type RollbackToken = String;

    async fn request(
        &self,
        input: FixedWindowInput,
    ) -> actix_web::Result<(bool, Self::Output, Self::RollbackToken)> {
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
        let output = FixedWindowOutput {
            limit: input.max_requests,
            remaining: input.max_requests.saturating_sub(count),
            reset: expiry,
        };
        Ok((allow, output, input.key))
    }

    async fn rollback(&self, token: Self::RollbackToken) -> actix_web::Result<()> {
        self.map.entry(token).and_modify(|v| {
            v.count = v.count.saturating_sub(1);
        });
        Ok(())
    }
}

#[async_trait(?Send)]
impl FixedWindowBackend for FixedWindowInMemory {
    async fn remove_key(&self, key: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.map.remove(key);
        Ok(())
    }
}

impl Drop for FixedWindowInMemory {
    fn drop(&mut self) {
        self.gc_handle.abort();
    }
}

pub struct FixedWindowInMemoryBuilder {
    gc_interval: Duration,
}

impl FixedWindowInMemoryBuilder {
    /// Override the default garbage collector interval.
    ///
    /// The garbage collector periodically scans the internal map, removing expired buckets.
    pub fn with_gc_interval(mut self, interval: Duration) -> Self {
        self.gc_interval = interval;
        self
    }

    pub fn build(self) -> FixedWindowInMemory {
        let map = Arc::new(DashMap::<String, Value>::new());
        let gc_handle = Arc::new(FixedWindowInMemory::garbage_collector(
            map.clone(),
            self.gc_interval,
        ));
        FixedWindowInMemory { map, gc_handle }
    }
}
