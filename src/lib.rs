//! Rate limiting middleware for actix-web
//!
//! # Getting Started:
//! ```no_run
//! # use actix_extensible_rate_limit::backend::{memory::InMemoryBackend, SimpleInputFunctionBuilder};
//! # use actix_extensible_rate_limit::RateLimiter;
//! # use actix_web::{App, HttpServer};
//! # use std::time::Duration;
//! #[actix_web::main]
//! async fn main() -> std::io::Result<()> {
//!     // A backend is responsible for storing rate limit data, and choosing whether to allow/deny requests
//!     let backend = InMemoryBackend::builder().build();
//!     HttpServer::new(move || {
//!         // Assign a limit of 5 requests per minute per client ip address
//!         let input = SimpleInputFunctionBuilder::new(Duration::from_secs(60), 5)
//!             .real_ip_key()
//!             .build();
//!         let middleware = RateLimiter::builder(backend.clone(), input)
//!             .add_headers()
//!             .build();
//!         App::new().wrap(middleware)
//!     })
//!     .bind("127.0.0.1:8080")?
//!     .run()
//!     .await
//! }
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod backend;
mod middleware;

pub use middleware::builder::{HeaderCompatibleOutput, RateLimiterBuilder};
pub use middleware::RateLimiter;
