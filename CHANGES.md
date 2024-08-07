# Changes

## 0.4.0 2024-08-07

- Major: Update Dashmap and Redis dependencies.

## 0.3.1 2024-01-21

- Patch: Fix Redis key expiry bug.

## 0.3.0 2024-01-21

- Major: Removes async-trait dependency.
- Major: Redis backend now uses BITFIELD to store counts.
- Major: Backend return type is now a `Decision` enum instead of a `bool`.

## 0.2.2 2022-04-19

- Patch: Improve documentation.

## 0.2.1 2022-04-18

- Minor: Added `SimpleBuilderFuture` type alias.

## 0.2.0 2022-04-17

- Major: Middleware will now always render a response on error cases, except for an error from the wrapped service call 
  which is now returned immediately.

## 0.1.1 2022-04-16

- Patch: Fixed docs.rs configuration
