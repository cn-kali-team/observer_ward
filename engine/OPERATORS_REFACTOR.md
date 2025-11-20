# Operators Refactoring - Request and Response Matching

## Overview

This document describes the refactoring of the operators module to support matching and extracting data from both HTTP Requests and Responses.

## Problem Statement

Previously, the operators could only match against HTTP Responses. For MITM proxy scenarios, we needed the ability to match against both HTTP Requests and Responses to enable comprehensive fingerprinting.

## Solution

We introduced a trait-based abstraction that allows operators to work with any type that can provide headers and body data.

## Architecture

### OperatorTarget Trait

The core of the refactoring is the `OperatorTarget` trait defined in `engine/src/operators/target.rs`:

```rust
pub trait OperatorTarget {
  fn get_headers(&self) -> String;
  fn get_body(&self) -> Option<Body>;
  fn get_header(&self, name: &str) -> Option<String>;
  fn get_full_content(&self) -> String;
  fn get_body_string(&self) -> String;
}
```

This trait is implemented for both:
- `slinger::Request` - HTTP request objects
- `slinger::Response` - HTTP response objects

### Generic Methods

The `Operators` struct now provides generic methods:

```rust
// Match against any OperatorTarget
pub fn matcher_generic<T: OperatorTarget>(
  &self,
  target: &T,
  response_for_extensions: Option<&Response>,
  result: &mut OperatorResult,
) -> Result<()>

// Extract from any OperatorTarget
pub fn extractor_generic<T: OperatorTarget>(
  &self,
  version: Option<Version>,
  target: &T,
  result: &mut OperatorResult,
)
```

### Convenience Methods

The `ClusteredOperator` provides convenient methods for different scenarios:

```rust
// Match against Response (existing behavior)
pub fn matcher(&self, results: &mut MatchEvent)

// Match against Request
pub fn matcher_request(&self, request: &Request, response: Option<&Response>, results: &mut MatchEvent)

// Match against both Request and Response
pub fn matcher_both(&self, request: &Request, response: &Response, results: &mut MatchEvent)
```

## Usage Examples

### Matching Against Request

```rust
use engine::slinger::Request;
use engine::operators::{Operators, OperatorResult};

let request = /* ... */;
let operators = /* ... */;
let mut result = OperatorResult::default();

// Match against the request
operators.matcher_generic(&request, None, &mut result)?;
```

### Matching Against Response (Backward Compatible)

```rust
use engine::slinger::Response;
use engine::operators::{Operators, OperatorResult};

let response = /* ... */;
let operators = /* ... */;
let mut result = OperatorResult::default();

// Existing code continues to work
operators.matcher(&response, &mut result)?;
```

### MITM Proxy Scenario

```rust
// In MITM proxy, match against both request and response
if let Some(request) = response.extensions().get::<Request>() {
  for cluster in clusters.iter() {
    for operator in cluster.operators.iter() {
      // Match against request
      operator.matcher_request(request, Some(&response), &mut result);
      
      // Match against response
      operator.matcher(&mut result);
    }
  }
}
```

## Response-Specific Features

Some matchers require Response-specific data:

- **Status Code Matching**: Only works with Response objects
- **Favicon Matching**: Requires Response extensions

When matching against a Request, these matchers gracefully return no match rather than failing.

## Testing

Comprehensive unit tests validate the functionality:

- `test_operator_target_trait_for_request` - Trait implementation for Request
- `test_operator_target_trait_for_response` - Trait implementation for Response
- `test_matcher_generic_with_request` - Request matching
- `test_matcher_generic_with_response` - Response matching

Run tests with: `cargo test --lib -p engine`

## Backward Compatibility

The refactoring maintains **100% backward compatibility**:

- All existing method signatures remain unchanged
- Existing code requires no modifications
- New functionality is opt-in through new methods
- All existing tests continue to pass

## Migration Guide

No migration required! Existing code continues to work without changes.

To use the new functionality:

1. Use `matcher_generic()` instead of `matcher()` to match against Request objects
2. Use `matcher_request()` in `ClusteredOperator` for convenience
3. Pass `None` for `response_for_extensions` when matching pure requests

## Performance Considerations

- No performance impact on existing code paths
- Generic methods are monomorphized at compile time (zero-cost abstraction)
- Same memory layout and access patterns as before

## Future Extensions

The trait-based design makes it easy to support additional target types:

- TCP packet data
- DNS queries/responses
- WebSocket messages
- gRPC requests/responses

Simply implement the `OperatorTarget` trait for the new type!
