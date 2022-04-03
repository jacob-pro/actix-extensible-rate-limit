pub mod backend;
mod middleware;
mod policy;

pub use middleware::builder::RateLimiterBuilder;
pub use middleware::RateLimiter;
pub use policy::Policy;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
