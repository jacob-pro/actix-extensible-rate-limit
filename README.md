# Actix Extensible Rate Limit

[![Build status](https://github.com/jacob-pro/actix-extensible-rate-limit/actions/workflows/rust.yml/badge.svg)](https://github.com/jacob-pro/actix-extensible-rate-limit/actions)
![maintenance-status](https://img.shields.io/badge/maintenance-experimental-blue.svg)

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
