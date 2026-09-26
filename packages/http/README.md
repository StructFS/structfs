# structfs-http

HTTP stores for StructFS.

## Stores

### HttpBrokerStore

Synchronous HTTP broker - write queues a request, read from handle executes it:

```rust
use structfs_http::{HttpBrokerStore, HttpRequest};
use structfs_core_store::{Reader, Writer, Record, path};
use structfs_serde_store::to_value;

let mut broker = HttpBrokerStore::with_default_timeout()?;

// Queue a request
let request = HttpRequest::get("https://api.example.com/users/1")
    .with_header("Authorization", "Bearer token");
let handle = broker.write(&path!(""), Record::parsed(to_value(&request)?))?;
// handle = "outstanding/0"

// Execute by reading
let record = broker.read(&handle)?.unwrap();
```

### BackgroundHttpBrokerStore

HTTP broker whose requests execute on background threads. The store itself
is synchronous — it implements `Reader`/`Writer`, not the async traits —
which is why it is named *background* rather than *async*:

```rust
use structfs_http::BackgroundHttpBrokerStore;

let mut broker = BackgroundHttpBrokerStore::with_default_timeout()?;

// Queue request (starts executing immediately)
let handle = broker.write(&path!(""), Record::parsed(to_value(&request)?))?;

// Check status
let status = broker.read(&handle)?;

// Get response when ready, or park until it is
let response = broker.read(&handle.join(&path!("response")))?;
let response = broker.read(&handle.join(&path!("response/wait")))?;
```

### HttpClientStore

HTTP client with a fixed base URL:

```rust
use structfs_http::HttpClientStore;

let mut client = HttpClientStore::new("https://api.example.com")?;

// GET /users/123
let record = client.read(&path!("users/123"))?;

// POST a value as the body. At the root, a map with a `method` or `path`
// key is instead a request spec, executed as written — and rejected with
// InvalidArgument if it does not parse (a typo is never POSTed). Any other
// value, including `{}`, is POSTed to the base URL. To POST a body that has
// its own `method` or `path` field, wrap it:
// {"method": "POST", "body": {"path": "/a", "name": "x"}}.
client.write(&path!("users"), Record::parsed(to_value(&data)?))?;
```

## The execution seam

Every blocking store sends its requests through one trait:

```rust
pub trait BlockingHttpExecutor: Send + Sync {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, Error>;
}
```

`BlockingReqwestExecutor` is the production implementation and reuses a
single pooled client. Pass your own to `with_executor` to test without a
network or to supply a different transport. The async, streaming counterpart
is `streaming::AsyncHttpExecutor`.

## Runtime constraints

- Building a blocking reqwest client **panics inside a Tokio runtime**.
  Construct `HttpBrokerStore`, `HttpClientStore` and the default
  `BackgroundHttpBrokerStore` off the runtime, or supply your own executor.
- `BackgroundHttpBrokerStore` **owns a small Tokio runtime** and drives its
  handle store with `block_on`, so it must not be called from inside another
  runtime either. Call it from ordinary or blocking threads.

## Errors

HTTP failures reach store callers as typed `structfs_core_store::Error`
variants rather than strings: timeouts are `DeadlineExceeded`; a connection
the peer *refused or reset* is `Overloaded` (retryable); 401/403 are
`PermissionDenied`; malformed, relative or non-http(s) URLs and bad methods
are `InvalidArgument`; and a 404 on a write is `NotFound` for the store path.
Other connect failures — DNS resolution, TLS handshake, certificate errors —
are not treated as transient and stay `Store` with their message. A 404 on a
*read* is `Ok(None)`, per the `Reader::read` contract. The full table is on
the `error` module. Status-to-error mapping applies only to `HttpClientStore`;
the brokers return non-2xx responses as ordinary `Ok` `HttpResponse` values.

Response headers (on `HttpResponse` and `StreamingResponse`) are a
single-valued map: a repeated header such as `Set-Cookie` collapses to the
last value received.

## Types

### HttpRequest

```rust
pub struct HttpRequest {
    pub method: Method,           // GET, POST, PUT, DELETE, CONNECT, TRACE, …
    pub path: String,             // URL or path
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Option<serde_json::Value>,
}

// Builder pattern (the type is #[non_exhaustive]: build, don't struct-literal)
let req = HttpRequest::post("https://api.example.com/data")
    .with_header("Content-Type", "application/json")
    .with_query("version", "2")
    .with_body(&data)?;
```

### HttpResponse

```rust
pub struct HttpResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: HashMap<String, String>,
    pub body: serde_json::Value,
    pub body_text: Option<String>,
}

let response = HttpResponse::new(201)          // canonical reason phrase
    .with_header("Location", "/users/1")
    .with_json_body(serde_json::json!({"id": 1}));

// Helper methods
response.is_success();      // 2xx
response.is_client_error(); // 4xx
response.is_server_error(); // 5xx
```

## Features

- `blocking` (default): the three stores above plus
  `BlockingReqwestExecutor`, over reqwest's blocking client.
- `streaming`: `streaming::AsyncReqwestExecutor` — an incremental,
  pull-based response body (see "Incremental responses" below).

## Portable types

`default-features = false` exposes HTTP request/response/status types and portable
errors. The default `blocking` feature adds native HTTP stores,
`BlockingReqwestExecutor`, and native executor dependencies. Keep it behind
target-specific dependencies in browser-shared code. See the
[platform matrix](../../docs/platforms.md).

## Incremental responses

Enable `streaming` for `streaming::AsyncReqwestExecutor`. It implements
`AsyncHttpExecutor`, returning status/headers before a pull-based `ByteStream`.
No `reqwest` or `http` type appears in a public signature: `StreamingResponse`
reports the same `u16` status and `HashMap` headers as `HttpResponse`.
Dropping the response abandons the body; `read_limited` explicitly buffers at most
a configured byte count, useful for bounded error responses. Query parameters,
headers, method and optional JSON body are forwarded.

`sse::SseFramer` is independently available without native HTTP. It handles chunk
and UTF-8 boundaries, multiline data, CRLF/LF/CR and comments with a configured
frame limit. EOF emits pending data. Errors are terminal but completed frames
before a later error remain in output order. It has no provider dialect or
reconnection policy. Budget output queues separately from per-frame retention.
