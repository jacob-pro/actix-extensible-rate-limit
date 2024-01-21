# Actix Extensible Rate Limit

[![Build status](https://github.com/jacob-pro/actix-extensible-rate-limit/actions/workflows/rust.yml/badge.svg)](https://github.com/jacob-pro/actix-extensible-rate-limit/actions)
[![crates.io](https://img.shields.io/crates/v/actix-extensible-rate-limit.svg)](https://crates.io/crates/actix-extensible-rate-limit)
[![docs.rs](https://docs.rs/actix-extensible-rate-limit/badge.svg)](https://docs.rs/actix-extensible-rate-limit/latest/actix_extensible_rate_limit/)

An attempt at a more flexible rate limiting middleware for actix-web

Allows for:

- Deriving a custom rate limit key from the request context.
- Using dynamic rate limits and intervals determined by the request context.
- Using custom backends (store & algorithm)
- Setting a custom 429 response.
- Transforming the response headers based on rate limit results (e.g `x-ratelimit-remaining`).
- Rolling back rate limit counts based on response codes.

## Provided Backends

| Backend         | Algorithm    | Store                                          |
|-----------------|--------------|------------------------------------------------|
| InMemoryBackend | Fixed Window | [Dashmap](https://github.com/xacrimon/dashmap) |
| RedisBackend    | Fixed Window | [Redis](https://github.com/mitsuhiko/redis-rs) |

## Getting Started

```rust
use actix_web::{App, HttpServer};
use actix_extensible_rate_limit::{
    backend::{memory::InMemoryBackend, SimpleInputFunctionBuilder},
    RateLimiter,
};
use std::time::Duration;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // A backend is responsible for storing rate limit data, and choosing whether to allow/deny requests
    let backend = InMemoryBackend::builder().build();

    HttpServer::new(move || {
        // Assign a limit of 5 requests per minute per client ip address
        let input = SimpleInputFunctionBuilder::new(Duration::from_secs(60), 5)
            .real_ip_key()
            .build();
        let middleware = RateLimiter::builder(backend.clone(), input)
            .add_headers()
            .build();
        App::new().wrap(middleware)
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}
```

Try it out:

```
$ curl -v http://127.0.0.1:8080
*   Trying 127.0.0.1:8080...
* Connected to 127.0.0.1 (127.0.0.1) port 8080 (#0)
> GET / HTTP/1.1
> Host: 127.0.0.1:8080
> User-Agent: curl/7.83.1
> Accept: */*
>
* Mark bundle as not supporting multiuse
< HTTP/1.1 404 Not Found
< content-length: 0
< x-ratelimit-limit: 5
< x-ratelimit-reset: 60
< x-ratelimit-remaining: 4
< date: Sun, 21 Jan 2024 16:52:27 GMT
<
* Connection #0 to host 127.0.0.1 left intact
```