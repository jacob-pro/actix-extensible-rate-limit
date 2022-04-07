pub mod backend;
mod middleware;

pub use middleware::builder::{HeaderCompatibleOutput, RateLimiterBuilder};
pub use middleware::RateLimiter;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
