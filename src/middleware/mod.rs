pub mod builder;

use crate::backend::Backend;
use actix_utils::future::{ok, Ready};
use actix_web::body::EitherBody;
use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::http::header::HeaderMap;
use actix_web::http::StatusCode;
use actix_web::HttpResponse;
use builder::RateLimiterBuilder;
use std::cell::RefCell;
use std::{future::Future, pin::Pin, rc::Rc};

type AllowedTransformation<BO> = dyn Fn(&mut HeaderMap, Option<&BO>);
type DeniedResponse<BO> = dyn Fn(&BO) -> HttpResponse;
type RollbackCondition = dyn Fn(StatusCode) -> bool;

/// Rate limit middleware.
pub struct RateLimiter<BE, BO, F> {
    backend: BE,
    input_fn: Rc<F>,
    fail_open: bool,
    allowed_mutation: Option<Rc<AllowedTransformation<BO>>>,
    denied_response: Rc<DeniedResponse<BO>>,
    rollback_condition: Option<Rc<RollbackCondition>>,
}

impl<BE, BI, BO, F, O> Clone for RateLimiter<BE, BO, F>
where
    BE: Backend<BI> + 'static,
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

impl<BE, BI, BO, F, O> RateLimiter<BE, BO, F>
where
    BE: Backend<BI, Output = BO> + 'static,
    BI: 'static,
    F: Fn(&ServiceRequest) -> O + 'static,
    O: Future<Output = Result<BI, actix_web::Error>>,
{
    /// # Arguments
    ///
    /// * `backend`: A rate limiting algorithm and store implementation.
    /// * `input_fn`: A future that produces input to the backend based on the incoming request.
    pub fn builder(backend: BE, input_fn: F) -> RateLimiterBuilder<BE, BO, F> {
        RateLimiterBuilder::new(backend, input_fn)
    }
}

impl<S, B, BE, BI, BO, F, O> Transform<S, ServiceRequest> for RateLimiter<BE, BO, F>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = actix_web::Error> + 'static,
    S::Future: 'static,
    B: 'static,
    BE: Backend<BI, Output = BO> + 'static,
    BI: 'static,
    BO: 'static,
    F: Fn(&ServiceRequest) -> O + 'static,
    O: Future<Output = Result<BI, actix_web::Error>>,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = actix_web::Error;
    type Transform = RateLimiterMiddleware<S, BE, BO, F>;
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

impl<S, B, BE, BI, BO, F, O> Service<ServiceRequest> for RateLimiterMiddleware<S, BE, BO, F>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = actix_web::Error> + 'static,
    S::Future: 'static,
    B: 'static,
    BE: Backend<BI, Output = BO> + 'static,
    BI: 'static,
    BO: 'static,
    F: Fn(&ServiceRequest) -> O + 'static,
    O: Future<Output = Result<BI, actix_web::Error>>,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = actix_web::Error;
    #[allow(clippy::type_complexity)]
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

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
            let input = (input_fn)(&req).await?;

            let output = match backend.request(input).await {
                // Able to successfully query rate limiter backend
                Ok((allow, output)) => {
                    if !allow {
                        let response: HttpResponse = (denied_response)(&output);
                        return Ok(req.into_response(response).map_into_right_body());
                    }
                    Some(output)
                }
                // Unable to query rate limiter backend
                Err(e) => {
                    if fail_open {
                        log::warn!("Rate limiter failed: {}, allowing the request anyway", e);
                        None
                    } else {
                        log::error!("Rate limiter failed: {}", e);
                        return Err(e);
                    }
                }
            };

            let mut service_response = service.call(req).await?;

            if let Some(transformation) = allowed_transformation {
                (transformation)(service_response.headers_mut(), output.as_ref());
            }

            // Can only rollback if the rate limiter was working in the first place.
            if let Some(output) = output {
                if let Some(rollback_condition) = rollback_condition {
                    let status = service_response.status();
                    if rollback_condition(status) {
                        if let Err(e) = backend.rollback(output).await {
                            log::error!("Unable to rollback rate-limit count for response with code: {status}, error: {e}");
                        };
                    }
                }
            }

            Ok(ServiceResponse::map_into_left_body(service_response))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::FixedWindowInMemory;
    use crate::backend::FixedWindowInput;
    use actix_web::http::StatusCode;
    use actix_web::test::TestRequest;
    use actix_web::{get, test, App, HttpResponse, Responder};
    use std::time::Duration;

    #[get("/")]
    async fn success() -> impl Responder {
        HttpResponse::Ok().body("Hello world!")
    }

    #[actix_web::test]
    async fn test_index_get() {
        let backend = FixedWindowInMemory::builder().build();
        let limiter = RateLimiter::builder(backend, |_req| async {
            Ok(FixedWindowInput {
                interval: Duration::from_secs(3600),
                max_requests: 2,
                key: "".to_string(),
            })
        })
        .add_headers()
        .build();
        let app = test::init_service(App::new().service(success).wrap(limiter)).await;
        assert!(
            test::call_service(&app, TestRequest::default().to_request())
                .await
                .status()
                .is_success()
        );
        assert!(
            test::call_service(&app, TestRequest::default().to_request())
                .await
                .status()
                .is_success()
        );
        assert_eq!(
            test::call_service(&app, TestRequest::default().to_request())
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
}
