use crate::backend::Backend;
use crate::middleware::{DeniedResponse, RateLimitStatus, RateLimiter, SuccessMutation};
use actix_web::dev::ServiceRequest;
use actix_web::http::header::{HeaderMap, HeaderName, HeaderValue, RETRY_AFTER};
use actix_web::HttpResponse;
use once_cell::sync::Lazy;
use std::rc::Rc;

pub static X_RATELIMIT_LIMIT: Lazy<HeaderName> =
    Lazy::new(|| HeaderName::from_static("x-ratelimit-limit"));

pub static X_RATELIMIT_REMAINING: Lazy<HeaderName> =
    Lazy::new(|| HeaderName::from_static("x-ratelimit-remaining"));

pub static X_RATELIMIT_RESET: Lazy<HeaderName> =
    Lazy::new(|| HeaderName::from_static("x-ratelimit-reset"));

pub struct RateLimiterBuilder<BE, F> {
    backend: BE,
    policy_fn: F,
    fail_open: bool,
    allowed_mutation: Option<Rc<SuccessMutation>>,
    denied_response: Rc<DeniedResponse>,
}

impl<BE, F, O> RateLimiterBuilder<BE, F>
where
    BE: Backend + 'static,
    F: Fn(&ServiceRequest) -> O,
{
    pub(super) fn new(backend: BE, policy_fn: F) -> Self {
        Self {
            backend,
            policy_fn,
            fail_open: false,
            allowed_mutation: None,
            denied_response: Rc::new(|_| HttpResponse::TooManyRequests().finish()),
        }
    }

    /// Choose whether to allow a request if the backend returns a failure
    pub fn fail_open(mut self, fail_open: bool) -> Self {
        self.fail_open = fail_open;
        self
    }

    /// Sets the [allowed_mutation()](RateLimiterBuilder::allowed_mutation) and
    /// [denied_response()](RateLimiterBuilder::denied_response) functions, such that the
    /// following headers are set in both the allowed and denied responses:
    ///
    /// - `x-ratelimit-limit`\
    /// - `x-ratelimit-remaining`\
    /// - `x-ratelimit-reset` (seconds until the reset)
    /// - `Retry-After` (denied only, seconds until the reset)
    pub fn add_headers(mut self) -> Self {
        self.allowed_mutation = Some(Rc::new(|map, status| {
            if let Some(status) = status {
                map.insert(X_RATELIMIT_LIMIT.clone(), HeaderValue::from(status.limit));
                map.insert(
                    X_RATELIMIT_REMAINING.clone(),
                    HeaderValue::from(status.remaining),
                );
                map.insert(
                    X_RATELIMIT_RESET.clone(),
                    HeaderValue::from(status.seconds_until_reset()),
                );
            }
        }));
        self.denied_response = Rc::new(|status| {
            let mut response = HttpResponse::TooManyRequests().finish();
            let map = response.headers_mut();
            map.insert(X_RATELIMIT_LIMIT.clone(), HeaderValue::from(status.limit));
            map.insert(
                X_RATELIMIT_REMAINING.clone(),
                HeaderValue::from(status.remaining),
            );
            let seconds = status.seconds_until_reset();
            map.insert(X_RATELIMIT_RESET.clone(), HeaderValue::from(seconds));
            map.insert(RETRY_AFTER, HeaderValue::from(seconds));
            response
        });
        self
    }

    /// In the event that the request is allowed:
    ///
    /// You can optionally mutate the response headers to include the rate limit status.
    ///
    /// By default no changes are made to the response.
    ///
    /// Note the [RateLimitStatus] will be [None] if the backend failed (and this was allowed).
    pub fn allowed_mutation<M>(mut self, mutation: Option<M>) -> Self
    where
        M: Fn(&mut HeaderMap, Option<&RateLimitStatus>) + 'static,
    {
        self.allowed_mutation = mutation.map(|m| Rc::new(m) as Rc<SuccessMutation>);
        self
    }

    /// In the event that the request is denied, configure the [HttpResponse] returned.
    ///
    /// This defaults to an empty body with status 429.
    pub fn denied_response<R>(mut self, denied_response: R) -> Self
    where
        R: Fn(&RateLimitStatus) -> HttpResponse + 'static,
    {
        self.denied_response = Rc::new(denied_response);
        self
    }

    pub fn build(self) -> RateLimiter<BE, F> {
        RateLimiter {
            backend: self.backend,
            policy_fn: Rc::new(self.policy_fn),
            fail_open: self.fail_open,
            allowed_mutation: self.allowed_mutation,
            denied_response: self.denied_response,
        }
    }
}
