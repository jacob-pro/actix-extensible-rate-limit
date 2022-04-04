use std::time::Duration;

pub struct Policy {
    /// The rate limiting interval.
    pub interval: Duration,
    /// The total requests to be allowed within the interval.
    pub max_requests: usize,
    /// The rate limit key to be used for this request.
    pub key: String,
}
