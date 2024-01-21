# Changes

## 0.3.0 2024-01-21

- Breaking: Removes async-trait dependency.
- Breaking: Redis backend now uses BITFIELD to store counts.
- Breaking: Backend return type is now a `Decision` enum instead of a `bool`.

## 0.2.2 2022-04-19

- Improve documentation.

## 0.2.1 2022-04-18

- Added `SimpleBuilderFuture` type alias.

## 0.2.0 2022-04-17

- Middleware will now always render a response on error cases, except for an error from the wrapped service call which
  is now returned immediately.

## 0.1.1 2022-04-16

- Fixed docs.rs configuration

