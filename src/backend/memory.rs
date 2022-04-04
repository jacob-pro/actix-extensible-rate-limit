use crate::backend::Backend;
use actix_web::rt::task::JoinHandle;
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const DEFAULT_GC_INTERVAL_SECONDS: u64 = 60 * 10;

#[derive(Clone)]
pub struct InMemoryBackend {
    map: Arc<DashMap<String, Value>>,
    gc_handle: Arc<JoinHandle<()>>,
}

struct Value {
    ttl: Instant,
    count: usize,
}

impl InMemoryBackend {
    pub fn builder() -> InMemoryBackendBuilder {
        InMemoryBackendBuilder {
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

#[async_trait]
impl Backend for InMemoryBackend {
    async fn get_and_increment(&self, key: &str, interval: Duration) -> (usize, Instant) {
        let now = Instant::now();
        let mut count = 1;
        let mut expiry = now
            .checked_add(interval)
            .expect("Interval unexpectedly large");
        self.map
            .entry(key.to_string())
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
        (count, expiry)
    }

    async fn decrement(&self, key: &str) {
        self.map.entry(key.to_string()).and_modify(|v| {
            v.count = v.count.saturating_sub(1);
        });
    }
}

impl Drop for InMemoryBackend {
    fn drop(&mut self) {
        self.gc_handle.abort();
    }
}

pub struct InMemoryBackendBuilder {
    gc_interval: Duration,
}

impl InMemoryBackendBuilder {
    /// Override the default garbage collector interval.
    ///
    /// The garbage collector periodically scans the internal map, removing expired buckets.
    pub fn with_gc_interval(mut self, interval: Duration) -> Self {
        self.gc_interval = interval;
        self
    }

    pub fn build(self) -> InMemoryBackend {
        let map = Arc::new(DashMap::<String, Value>::new());
        let gc_handle = Arc::new(InMemoryBackend::garbage_collector(
            map.clone(),
            self.gc_interval,
        ));
        InMemoryBackend { map, gc_handle }
    }
}
