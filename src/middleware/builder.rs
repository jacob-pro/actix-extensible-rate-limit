use crate::backend::Backend;
use crate::middleware::RateLimiter;
use crate::Policy;
use actix_web::dev::ServiceRequest;
use std::future::Future;
use std::rc::Rc;

#[derive(Debug)]
pub struct RateLimiterBuilder<BE, F, O>
where
    BE: Backend + 'static,
    F: Fn(&ServiceRequest) -> O,
    O: Future<Output = Result<Policy, actix_web::Error>>,
{
    backend: BE,
    policy_fn: F,
    fail_open: bool,
}

impl<BE, F, O> RateLimiterBuilder<BE, F, O>
where
    BE: Backend + 'static,
    F: Fn(&ServiceRequest) -> O,
    O: Future<Output = Result<Policy, actix_web::Error>>,
{
    pub(super) fn new(backend: BE, policy_fn: F) -> Self {
        Self {
            backend,
            policy_fn,
            fail_open: false,
        }
    }

    /// Choose whether to allow a request if the backend returns a failure (e.g. Redis is down)
    pub fn fail_open(mut self, fail_open: bool) -> Self {
        self.fail_open = fail_open;
        self
    }

    pub fn build(self) -> RateLimiter<BE, F, O> {
        RateLimiter {
            backend: self.backend,
            policy_fn: Rc::new(self.policy_fn),
            fail_open: self.fail_open,
        }
    }
}
