use crate::backend::Backend;
use crate::middleware::{AllowedTransformation, DeniedResponse, RateLimiter, RollbackCondition};
use actix_web::dev::ServiceRequest;
use actix_web::http::header::{HeaderMap, HeaderName, HeaderValue, RETRY_AFTER};
use actix_web::http::StatusCode;
use actix_web::HttpResponse;
use once_cell::sync::Lazy;
use std::future::Future;
use std::rc::Rc;

pub static X_RATELIMIT_LIMIT: Lazy<HeaderName> =
    Lazy::new(|| HeaderName::from_static("x-ratelimit-limit"));

pub static X_RATELIMIT_REMAINING: Lazy<HeaderName> =
    Lazy::new(|| HeaderName::from_static("x-ratelimit-remaining"));

pub static X_RATELIMIT_RESET: Lazy<HeaderName> =
    Lazy::new(|| HeaderName::from_static("x-ratelimit-reset"));

pub struct RateLimiterBuilder<BE, BO, F> {
    backend: BE,
    input_fn: F,
    fail_open: bool,
    allowed_transformation: Option<Rc<AllowedTransformation<BO>>>,
    denied_response: Rc<DeniedResponse<BO>>,
    rollback_condition: Option<Rc<RollbackCondition>>,
}

impl<BE, BI, BO, F, O> RateLimiterBuilder<BE, BO, F>
where
    BE: Backend<BI, Output = BO> + 'static,
    BI: 'static,
    F: Fn(&ServiceRequest) -> O,
    O: Future<Output = Result<BI, actix_web::Error>>,
{
    pub(super) fn new(backend: BE, input_fn: F) -> Self {
        Self {
            backend,
            input_fn,
            fail_open: false,
            allowed_transformation: None,
            denied_response: Rc::new(|_| HttpResponse::TooManyRequests().finish()),
            rollback_condition: None,
        }
    }

    /// Choose whether to allow a request if the backend returns a failure.
    ///
    /// Default is false.
    pub fn fail_open(mut self, fail_open: bool) -> Self {
        self.fail_open = fail_open;
        self
    }

    /// Sets the [RateLimiterBuilder::request_allowed_transformation] and
    /// [RateLimiterBuilder::request_denied_response] functions, such that the following headers
    /// are set in both the allowed and denied responses:
    ///
    /// - `x-ratelimit-limit`\
    /// - `x-ratelimit-remaining`\
    /// - `x-ratelimit-reset` (seconds until the reset)
    /// - `retry-after` (denied only, seconds until the reset)
    ///
    /// This function requires the Backend Output to implement [HeaderCompatibleOutput]
    pub fn add_headers(mut self) -> Self
    where
        BO: HeaderCompatibleOutput,
    {
        self.allowed_transformation = Some(Rc::new(|map, output| {
            if let Some(status) = output {
                map.insert(X_RATELIMIT_LIMIT.clone(), HeaderValue::from(status.limit()));
                map.insert(
                    X_RATELIMIT_REMAINING.clone(),
                    HeaderValue::from(status.remaining()),
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
            map.insert(X_RATELIMIT_LIMIT.clone(), HeaderValue::from(status.limit()));
            map.insert(
                X_RATELIMIT_REMAINING.clone(),
                HeaderValue::from(status.remaining()),
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
    /// Note the [Backend::Output] will be [None] if the backend failed and
    /// [RateLimiterBuilder::fail_open] is enabled.
    pub fn request_allowed_transformation<M>(mut self, mutation: Option<M>) -> Self
    where
        M: Fn(&mut HeaderMap, Option<&BO>) + 'static,
    {
        self.allowed_transformation = mutation.map(|m| Rc::new(m) as Rc<AllowedTransformation<BO>>);
        self
    }

    /// In the event that the request is denied, configure the [HttpResponse] returned.
    ///
    /// Defaults to an empty body with status 429.
    pub fn request_denied_response<R>(mut self, denied_response: R) -> Self
    where
        R: Fn(&BO) -> HttpResponse + 'static,
    {
        self.denied_response = Rc::new(denied_response);
        self
    }

    /// After processing a request, attempt to rollback the request count based on the status code
    /// of the returned response.
    ///
    /// By default the rate limit is never rolled back.
    pub fn rollback_condition<C>(mut self, condition: Option<C>) -> Self
    where
        C: Fn(StatusCode) -> bool + 'static,
    {
        self.rollback_condition = condition.map(|m| Rc::new(m) as Rc<RollbackCondition>);
        self
    }

    /// Configures the [RateLimiterBuilder::rollback_condition] to rollback if the status code
    /// is a server error (5xx).
    pub fn rollback_server_errors(mut self) -> Self {
        self.rollback_condition = Some(Rc::new(|status| status.is_server_error()));
        self
    }

    pub fn build(self) -> RateLimiter<BE, BO, F> {
        RateLimiter {
            backend: self.backend,
            input_fn: Rc::new(self.input_fn),
            fail_open: self.fail_open,
            allowed_mutation: self.allowed_transformation,
            denied_response: self.denied_response,
            rollback_condition: self.rollback_condition,
        }
    }
}

/// A trait that a [Backend::Output] should implement in order to use the
/// [RateLimiterBuilder::add_headers] function.
pub trait HeaderCompatibleOutput {
    /// Value for the `x-ratelimit-limit` header.
    fn limit(&self) -> u64;

    /// Value for the `x-ratelimit-remaining` header.
    fn remaining(&self) -> u64;

    /// Value for the `x-ratelimit-reset` and `retry-at` headers.
    ///
    /// This should be the number of seconds from now until the limit resets.\
    /// If the limit has already reset this should return 0.
    fn seconds_until_reset(&self) -> u64;
}
