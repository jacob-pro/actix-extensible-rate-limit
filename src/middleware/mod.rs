pub mod builder;

use crate::backend::Backend;
use crate::Policy;
use actix_utils::future::{ok, Ready};
use actix_web::body::EitherBody;
use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::http::StatusCode;
use actix_web::HttpResponse;
use builder::RateLimiterBuilder;
use std::{future::Future, pin::Pin, rc::Rc};

/// Rate limit middleware.
#[derive(Debug)]
pub struct RateLimiter<BE, F, O>
where
    BE: Backend + 'static,
    F: Fn(&ServiceRequest) -> O + 'static,
    O: Future<Output = Result<Policy, actix_web::Error>>,
{
    backend: BE,
    policy_fn: Rc<F>,
    fail_open: bool,
}

impl<BE, F, O> Clone for RateLimiter<BE, F, O>
where
    BE: Backend + 'static,
    F: Fn(&ServiceRequest) -> O + 'static,
    O: Future<Output = Result<Policy, actix_web::Error>>,
{
    fn clone(&self) -> Self {
        Self {
            backend: self.backend.clone(),
            policy_fn: Rc::clone(&self.policy_fn),
            fail_open: self.fail_open,
        }
    }
}

impl<BE, F, O> RateLimiter<BE, F, O>
where
    BE: Backend + 'static,
    F: Fn(&ServiceRequest) -> O + 'static,
    O: Future<Output = Result<Policy, actix_web::Error>>,
{
    pub fn builder(backend: BE, policy_fn: F) -> RateLimiterBuilder<BE, F, O> {
        RateLimiterBuilder::new(backend, policy_fn)
    }
}

impl<S, B, BE, F, O> Transform<S, ServiceRequest> for RateLimiter<BE, F, O>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = actix_web::Error> + 'static,
    S::Future: 'static,
    B: 'static,
    BE: Backend + 'static,
    F: Fn(&ServiceRequest) -> O + 'static,
    O: Future<Output = Result<Policy, actix_web::Error>>,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = actix_web::Error;
    type Transform = RateLimiterMiddleware<S, BE, F>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ok(RateLimiterMiddleware {
            service: Rc::new(service),
            backend: self.backend.clone(),
            policy_fn: Rc::clone(&self.policy_fn),
        })
    }
}

#[derive(Debug)]
pub struct RateLimiterMiddleware<S, BE, F> {
    service: Rc<S>,
    backend: BE,
    policy_fn: Rc<F>,
}

impl<S, B, BE, F, O> Service<ServiceRequest> for RateLimiterMiddleware<S, BE, F>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = actix_web::Error> + 'static,
    S::Future: 'static,
    B: 'static,
    BE: Backend + 'static,
    F: Fn(&ServiceRequest) -> O + 'static,
    O: Future<Output = Result<Policy, actix_web::Error>>,
{
    type Response = ServiceResponse<EitherBody<B>>;
    type Error = actix_web::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = Rc::clone(&self.service);
        let policy_fn = Rc::clone(&self.policy_fn);
        let backend = self.backend.clone();

        Box::pin(async move {
            let policy: Policy = (policy_fn)(&req).await?;
            let (count, _ttl) = backend
                .get_and_increment(&policy.key, policy.interval)
                .await;
            if count > policy.max_requests {
                return Ok(req.into_response(
                    HttpResponse::new(StatusCode::TOO_MANY_REQUESTS).map_into_right_body(),
                ));
            }

            let res = service
                .call(req)
                .await
                .map(ServiceResponse::map_into_left_body)?;

            Ok(res)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::memory::InMemoryBackend;
    use actix_web::test::TestRequest;
    use actix_web::{get, test, App, HttpResponse, Responder};
    use std::time::Duration;

    #[get("/")]
    async fn hello() -> impl Responder {
        HttpResponse::Ok().body("Hello world!")
    }

    #[actix_web::test]
    async fn test_index_get() {
        let backend = InMemoryBackend::builder().build();
        let limiter = RateLimiter::builder(backend, |_req| async {
            Ok(Policy {
                interval: Duration::from_secs(3600),
                max_requests: 2,
                key: "".to_string(),
            })
        })
        .build();
        let app = test::init_service(App::new().service(hello).wrap(limiter)).await;
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
