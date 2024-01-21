pub mod builder;
#[cfg(test)]
mod tests;

use crate::backend::Backend;
use actix_web::body::EitherBody;
use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::http::header::HeaderMap;
use actix_web::http::StatusCode;
use actix_web::HttpResponse;
use builder::RateLimiterBuilder;
use futures::future::{ok, LocalBoxFuture, Ready};
use std::cell::RefCell;
use std::{future::Future, rc::Rc};

type AllowedTransformation<BO> = dyn Fn(&mut HeaderMap, Option<&BO>, bool);
type DeniedResponse<BO> = dyn Fn(&BO) -> HttpResponse;
type RollbackCondition = dyn Fn(StatusCode) -> bool;

/// Rate limit middleware.
pub struct RateLimiter<BA, BO, F> {
    backend: BA,
    input_fn: Rc<F>,
    fail_open: bool,
    allowed_mutation: Option<Rc<AllowedTransformation<BO>>>,
    denied_response: Rc<DeniedResponse<BO>>,
    rollback_condition: Option<Rc<RollbackCondition>>,
}

impl<BA, BI, BO, F, O> Clone for RateLimiter<BA, BO, F>
where
    BA: Backend<BI> + 'static,
    BI: 'static,
    F: Fn(&ServiceRequest) -> O + 'static,
    O: Future<Output = Result<BI, actix_web::Error>>,
{
    fn clone(&self) -> Self {
        Self {
            backend: self.backend.clone(),
            input_fn: self.input_fn.clone(),
            fail_open: self.fail_open,
            allowed_mutation: self.allowed_mutation.clone(),
            denied_response: self.denied_response.clone(),
            rollback_condition: self.rollback_condition.clone(),
        }
    }
}

impl<BA, BI, BO, F, O> RateLimiter<BA, BO, F>
where
    BA: Backend<BI, Output = BO> + 'static,
    BI: 'static,
    F: Fn(&ServiceRequest) -> O + 'static,
    O: Future<Output = Result<BI, actix_web::Error>>,
{
    /// # Arguments
    ///
    /// * `backend`: A rate limiting algorithm and store implementation.
    /// * `input_fn`: A future that produces input to the backend based on the incoming request.
    pub fn builder(backend: BA, input_fn: F) -> RateLimiterBuilder<BA, BO, F> {
        RateLimiterBuilder::new(backend, input_fn)
    }
}

impl<S, B, BA, BI, BO, BE, F, O> Transform<S, ServiceRequest> for RateLimiter<BA, BO, F>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = actix_web::Error> + 'static,
    S::Future: 'static,
    B: 'static,
    BA: Backend<BI, Output = BO, Error = BE> + 'static,
    BI: 'static,
    BO: 'static,
    BE: Into<actix_web::Error> + std::fmt::Display + 'static,
    F: Fn(&ServiceRequest) -> O + 'static,
    O: Future<Output = Result<BI, actix_web::Error>>,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = actix_web::Error;
    type Transform = RateLimiterMiddleware<S, BA, BO, F>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ok(RateLimiterMiddleware {
            service: Rc::new(RefCell::new(service)),
            backend: self.backend.clone(),
            input_fn: Rc::clone(&self.input_fn),
            fail_open: self.fail_open,
            allowed_transformation: self.allowed_mutation.clone(),
            denied_response: self.denied_response.clone(),
            rollback_condition: self.rollback_condition.clone(),
        })
    }
}

pub struct RateLimiterMiddleware<S, BE, BO, F> {
    service: Rc<RefCell<S>>,
    backend: BE,
    input_fn: Rc<F>,
    fail_open: bool,
    allowed_transformation: Option<Rc<AllowedTransformation<BO>>>,
    denied_response: Rc<DeniedResponse<BO>>,
    rollback_condition: Option<Rc<RollbackCondition>>,
}

impl<S, B, BA, BI, BO, BE, F, O> Service<ServiceRequest> for RateLimiterMiddleware<S, BA, BO, F>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = actix_web::Error> + 'static,
    S::Future: 'static,
    B: 'static,
    BA: Backend<BI, Output = BO, Error = BE> + 'static,
    BI: 'static,
    BO: 'static,
    BE: Into<actix_web::Error> + std::fmt::Display + 'static,
    F: Fn(&ServiceRequest) -> O + 'static,
    O: Future<Output = Result<BI, actix_web::Error>>,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = actix_web::Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = self.service.clone();
        let backend = self.backend.clone();
        let input_fn = self.input_fn.clone();
        let fail_open = self.fail_open;
        let allowed_transformation = self.allowed_transformation.clone();
        let denied_response = self.denied_response.clone();
        let rollback_condition = self.rollback_condition.clone();

        Box::pin(async move {
            let input = match (input_fn)(&req).await {
                Ok(input) => input,
                Err(e) => {
                    log::error!("Rate limiter input function failed: {e}");
                    return Ok(req.into_response(e.error_response()).map_into_right_body());
                }
            };

            let (output, rollback) = match backend.request(input).await {
                // Able to successfully query rate limiter backend
                Ok((decision, output, rollback)) => {
                    if decision.is_denied() {
                        let response: HttpResponse = (denied_response)(&output);
                        return Ok(req.into_response(response).map_into_right_body());
                    }
                    (Some(output), Some(rollback))
                }
                // Unable to query rate limiter backend
                Err(e) => {
                    if fail_open {
                        log::warn!("Rate limiter failed: {}, allowing the request anyway", e);
                        (None, None)
                    } else {
                        log::error!("Rate limiter failed: {}", e);
                        return Ok(req
                            .into_response(e.into().error_response())
                            .map_into_right_body());
                    }
                }
            };

            let mut service_response = service.call(req).await?;

            let mut rolled_back = false;
            if let Some(token) = rollback {
                if let Some(rollback_condition) = rollback_condition {
                    let status = service_response.status();
                    if rollback_condition(status) {
                        if let Err(e) = backend.rollback(token).await {
                            log::error!("Unable to rollback rate-limit count for response: {:?}, error: {e}", status);
                        } else {
                            rolled_back = true;
                        };
                    }
                }
            }

            if let Some(transformation) = allowed_transformation {
                (transformation)(service_response.headers_mut(), output.as_ref(), rolled_back);
            }

            Ok(service_response.map_into_left_body())
        })
    }
}
