use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::{
    cache::SessionCache, config::Config, protocol::ProtocolAdapter, registry::ConnectionRegistry,
    store::Store,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    pub adapter: Arc<dyn ProtocolAdapter>,
    pub registry: Arc<ConnectionRegistry>,
    pub cache: Arc<SessionCache>,
    pub limiter: Arc<RateLimiter>,
}

struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

/// Small in-process per-account token bucket used for action calls. This is an
/// implementation detail of the single-process prototype, not storage data.
pub struct RateLimiter {
    buckets: Mutex<HashMap<i64, Bucket>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn allow(&self, account_id: i64, rate_per_second: i32) -> bool {
        let rate = rate_per_second.max(1) as f64;
        let now = Instant::now();
        let mut buckets = self
            .buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let bucket = buckets.entry(account_id).or_insert(Bucket {
            tokens: rate,
            last_refill: now,
        });
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * rate).min(rate);
        bucket.last_refill = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}
