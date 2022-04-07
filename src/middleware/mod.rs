pub mod builder;

use crate::backend::Backend;
use actix_utils::future::{ok, Ready};
use actix_web::body::EitherBody;
use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::http::header::HeaderMap;
use actix_web::HttpResponse;
use builder::RateLimiterBuilder;
use std::cell::RefCell;
use std::{future::Future, pin::Pin, rc::Rc};

type SuccessMutation<BO> = dyn Fn(&mut HeaderMap, Option<&BO>);

type DeniedResponse<BO> = dyn Fn(&BO) -> HttpResponse;

/// Rate limit middleware.
pub struct RateLimiter<BE, BO, F> {
    backend: BE,
    policy_fn: Rc<F>,
    fail_open: bool,
    allowed_mutation: Option<Rc<SuccessMutation<BO>>>,
    denied_response: Rc<DeniedResponse<BO>>,
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
            policy_fn: self.policy_fn.clone(),
            fail_open: self.fail_open,
            allowed_mutation: self.allowed_mutation.clone(),
            denied_response: self.denied_response.clone(),
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
    pub fn builder(backend: BE, policy_fn: F) -> RateLimiterBuilder<BE, BO, F> {
        RateLimiterBuilder::new(backend, policy_fn)
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
            policy_fn: Rc::clone(&self.policy_fn),
            allowed_mutation: self.allowed_mutation.clone(),
            denied_response: self.denied_response.clone(),
        })
    }
}

pub struct RateLimiterMiddleware<S, BE, BO, F> {
    service: Rc<RefCell<S>>,
    backend: BE,
    policy_fn: Rc<F>,
    allowed_mutation: Option<Rc<SuccessMutation<BO>>>,
    denied_response: Rc<DeniedResponse<BO>>,
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
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = Rc::clone(&self.service);
        let policy_fn = Rc::clone(&self.policy_fn);
        let backend = self.backend.clone();
        let success_mutation = self.allowed_mutation.clone();
        let denied_response = self.denied_response.clone();

        Box::pin(async move {
            let input = (policy_fn)(&req).await?;
            let (allow, output) = backend.request(input).await?;

            if !allow {
                let response: HttpResponse = (denied_response)(&output);
                return Ok(req.into_response(response).map_into_right_body());
            }

            // The service can return either a ServiceResponse<UNKNOWN> or an actix_web:Error
            // In either case we still we may want to add the rate limiting headers.
            let service_response = match service.call(req).await {
                Ok(mut service_response) => {
                    if let Some(mutation) = success_mutation {
                        (mutation)(service_response.headers_mut(), Some(&output));
                    }
                    service_response
                }
                Err(e) => {
                    // We need to dismantle and re-assemble the error
                    let mut err_response: HttpResponse = e.error_response();
                    if let Some(mutation) = success_mutation {
                        (mutation)(err_response.headers_mut(), Some(&output));
                    }
                    // TODO: Fix this
                    return Err(e);
                }
            };

            Ok(ServiceResponse::map_into_left_body(service_response))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::fixed_window::FixedWindowInput;
    use crate::backend::memory::FixedWindowInMemory;
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
