# Actix Extensible Rate Limit

[![Build status](https://github.com/jacob-pro/actix-extensible-rate-limit/actions/workflows/rust.yml/badge.svg)](https://github.com/jacob-pro/actix-extensible-rate-limit/actions)
![maintenance-status](https://img.shields.io/badge/maintenance-experimental-blue.svg)

An attempt at a better rate limiting middleware for actix-web

Allows for:

- Deriving a custom rate limit key from the request context.
- Using dynamic rate limits and intervals determined by the request context.
- Using custom backends.
