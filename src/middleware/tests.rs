use crate::middleware::*;
use actix_web::http::header::{HeaderName, HeaderValue};
use actix_web::http::StatusCode;
use actix_web::test::{read_body, TestRequest};
use actix_web::{get, test, App, HttpResponse, Responder, ResponseError};
use async_trait::async_trait;
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[get("/200")]
async fn route_200() -> impl Responder {
    HttpResponse::Ok().body("Hello world!")
}

#[get("/500")]
async fn route_500() -> impl Responder {
    HttpResponse::InternalServerError().body("Internal error")
}

#[derive(Clone, Default)]
struct MockBackend(Arc<MockBackendInner>);

#[derive(Default)]
struct MockBackendInner {
    counter: AtomicU64,
}

struct MockBackendInput<T> {
    max: u64,
    output: T,
    backend_error: Option<actix_web::Error>,
}

#[async_trait(?Send)]
impl<T: 'static> Backend<MockBackendInput<T>> for MockBackend {
    type Output = T;
    type RollbackToken = ();

    async fn request(
        &self,
        input: MockBackendInput<T>,
    ) -> actix_web::Result<(bool, Self::Output, Self::RollbackToken)> {
        if let Some(e) = input.backend_error {
            return Err(e);
        }
        let allow = self.0.counter.fetch_add(1, Ordering::Relaxed) < input.max;
        Ok((allow, input.output, ()))
    }

    async fn rollback(&self, _: Self::RollbackToken) -> actix_web::Result<()> {
        self.0.counter.fetch_sub(1, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct MockError {
    code: StatusCode,
    message: String,
}

impl Default for MockError {
    fn default() -> Self {
        MockError {
            code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Mock Error".to_string(),
        }
    }
}

impl Display for MockError {
    fn fmt(&self, _: &mut Formatter<'_>) -> std::fmt::Result {
        unreachable!()
    }
}

impl ResponseError for MockError {
    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.code).body(self.message.clone())
    }
}

#[actix_web::test]
async fn test_allow_deny() {
    let backend = MockBackend::default();
    let limiter = RateLimiter::builder(backend, |_req| async {
        Ok(MockBackendInput {
            max: 1,
            output: (),
            backend_error: None,
        })
    })
    .build();
    let app = test::init_service(App::new().service(route_200).wrap(limiter)).await;
    assert!(
        test::call_service(&app, TestRequest::get().uri("/200").to_request())
            .await
            .status()
            .is_success()
    );
    assert_eq!(
        test::call_service(&app, TestRequest::get().uri("/200").to_request())
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[actix_web::test]
async fn test_custom_deny_response() {
    let backend = MockBackend::default();
    let limiter = RateLimiter::builder(backend, |_req| async {
        Ok(MockBackendInput {
            max: 0,
            output: StatusCode::IM_A_TEAPOT,
            backend_error: None,
        })
    })
    .request_denied_response(|output| HttpResponse::build(*output).body("Custom denied response"))
    .build();
    let app = test::init_service(App::new().service(route_200).wrap(limiter)).await;
    let response = test::call_service(&app, TestRequest::get().uri("/200").to_request()).await;
    assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    let body = String::from_utf8(read_body(response).await.to_vec()).unwrap();
    assert_eq!(body, "Custom denied response");
}

#[actix_web::test]
async fn test_header_transformation() {
    let backend = MockBackend::default();
    let limiter = RateLimiter::builder(backend, |_req| async {
        Ok(MockBackendInput {
            max: u64::MAX,
            output: "abc".to_string(),
            backend_error: None,
        })
    })
    .request_allowed_transformation(Some(
        |headers: &mut HeaderMap, output: Option<&String>, rolled_back: bool| {
            assert!(!rolled_back);
            assert!(
                output.is_some(),
                "Backend is working so output should be some"
            );
            headers.insert(
                HeaderName::from_static("test-header"),
                HeaderValue::from_str(output.unwrap()).unwrap(),
            );
        },
    ))
    .build();
    let app = test::init_service(App::new().service(route_200).wrap(limiter)).await;
    let response = test::call_service(&app, TestRequest::get().uri("/200").to_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("test-header")
            .unwrap()
            .to_str()
            .unwrap(),
        "abc"
    );
}

#[actix_web::test]
async fn test_fail_open() {
    let backend = MockBackend::default();

    // Test first without fail open
    let limiter = RateLimiter::builder(backend.clone(), |_req| async {
        Ok(MockBackendInput {
            max: u64::MAX,
            output: (),
            backend_error: Some(MockError::default().into()),
        })
    })
    .build();
    let app = test::init_service(App::new().service(route_200).wrap(limiter)).await;
    assert!(app
        .call(TestRequest::get().uri("/200").to_request())
        .await
        .is_err());

    // Test again with fail open enabled
    let limiter = RateLimiter::builder(backend, |_req| async {
        Ok(MockBackendInput {
            max: u64::MAX,
            output: (),
            backend_error: Some(MockError::default().into()),
        })
    })
    .request_allowed_transformation(Some(
        |map: &mut HeaderMap, output: Option<&()>, rolled_back: bool| {
            assert!(!rolled_back);
            map.insert(
                HeaderName::from_static("custom-header"),
                HeaderValue::from_static(""),
            );
            assert!(output.is_none());
        },
    ))
    .fail_open(true)
    .build();
    let app = test::init_service(App::new().service(route_200).wrap(limiter)).await;
    let response = test::call_service(&app, TestRequest::get().uri("/200").to_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().contains_key("custom-header"))
}

#[actix_web::test]
async fn test_rollback() {
    let backend = MockBackend::default();
    let limiter = RateLimiter::builder(backend.clone(), |_req| async {
        Ok(MockBackendInput {
            max: u64::MAX,
            output: (),
            backend_error: None,
        })
    })
    .rollback_server_errors()
    .build();
    let app = test::init_service(
        App::new()
            .service(route_200)
            .service(route_500)
            .wrap(limiter),
    )
    .await;

    // Confirm count increases for a 200 response
    let response = test::call_service(&app, TestRequest::get().uri("/200").to_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(backend.0.counter.load(Ordering::Relaxed), 1);

    // Confirm count hasn't increased because of rollback
    let response = test::call_service(&app, TestRequest::get().uri("/500").to_request()).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(backend.0.counter.load(Ordering::Relaxed), 1);
}

#[actix_web::test]
async fn test_rollback_and_transform_when_service_errors() {
    let backend = MockBackend::default();

    let broken = broken_middleware::Broken(MockError {
        code: StatusCode::IM_A_TEAPOT,
        message: "Teapot".to_string(),
    });

    let limiter = RateLimiter::builder(backend.clone(), |_req| async {
        Ok(MockBackendInput {
            max: u64::MAX,
            output: (),
            backend_error: None,
        })
    })
    .request_allowed_transformation(Some(
        |headers: &mut HeaderMap, _: Option<&()>, rolled_back: bool| {
            assert!(rolled_back);
            headers.append(
                HeaderName::from_static("here"),
                HeaderValue::from_static("abc"),
            )
        },
    ))
    .rollback_server_errors()
    .build();

    let app = test::init_service(
        App::new()
            .service(actix_web::web::scope("").service(route_200).wrap(broken))
            .wrap(limiter),
    )
    .await;

    // The request should fail because the inner middleware failed.
    // But header should still be present indicating the outer rate limiter was working.
    let failure: HttpResponse = app
        .call(TestRequest::get().uri("/200").to_request())
        .await
        .unwrap_err()
        .as_response_error()
        .error_response();
    assert!(failure.headers().contains_key("here"));
    assert_eq!(failure.status(), StatusCode::IM_A_TEAPOT);
    // But the count should not have increased because of rollback
    assert_eq!(backend.0.counter.load(Ordering::Relaxed), 0);
}

mod broken_middleware {
    use super::MockError;
    use actix_web::{
        dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
        Error,
    };
    use std::future::{ready, Future, Ready};
    use std::pin::Pin;

    // A Middleware that allows us to test what happens when the service.call returns an error
    pub struct Broken(pub(super) MockError);

    impl<S, B> Transform<S, ServiceRequest> for Broken
    where
        S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
        S::Future: 'static,
        B: 'static,
    {
        type Response = ServiceResponse<B>;
        type Error = Error;
        type Transform = BrokenMiddleware<S>;
        type InitError = ();
        type Future = Ready<Result<Self::Transform, Self::InitError>>;

        fn new_transform(&self, service: S) -> Self::Future {
            ready(Ok(BrokenMiddleware {
                service,
                error: self.0.clone(),
            }))
        }
    }

    pub struct BrokenMiddleware<S> {
        service: S,
        error: MockError,
    }

    impl<S, B> Service<ServiceRequest> for BrokenMiddleware<S>
    where
        S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
        S::Future: 'static,
        B: 'static,
    {
        type Response = ServiceResponse<B>;
        type Error = Error;
        type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

        forward_ready!(service);

        fn call(&self, _: ServiceRequest) -> Self::Future {
            let err = self.error.clone();
            Box::pin(async move { Err(err.into()) })
        }
    }
}
