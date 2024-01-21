use crate::backend::Decision;
use crate::middleware::*;
use actix_web::http::header::{HeaderName, HeaderValue};
use actix_web::http::StatusCode;
use actix_web::test::{read_body, TestRequest};
use actix_web::{get, test, App, HttpResponse, Responder, ResponseError};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;

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
    backend_error: Option<MockError>,
}

impl<T: 'static> Backend<MockBackendInput<T>> for MockBackend {
    type Output = T;
    type RollbackToken = ();
    type Error = MockError;

    async fn request(
        &self,
        input: MockBackendInput<T>,
    ) -> Result<(Decision, Self::Output, Self::RollbackToken), Self::Error> {
        if let Some(e) = input.backend_error {
            return Err(e);
        }
        let allow = self.0.counter.fetch_add(1, Ordering::Relaxed) < input.max;
        Ok((Decision::from_allowed(allow), input.output, ()))
    }

    async fn rollback(&self, _: Self::RollbackToken) -> Result<(), Self::Error> {
        self.0.counter.fetch_sub(1, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Debug, Clone, Error)]
#[error("MockError: {message}")]
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

impl ResponseError for MockError {
    fn status_code(&self) -> StatusCode {
        self.code
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
    let response = test::call_service(&app, TestRequest::get().uri("/200").to_request()).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

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
