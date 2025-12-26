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

### AsyncHttpBrokerStore

Async HTTP broker - requests execute in background threads:

```rust
use structfs_http::AsyncHttpBrokerStore;

let mut broker = AsyncHttpBrokerStore::with_default_timeout()?;

// Queue request (starts executing immediately)
let handle = broker.write(&path!(""), Record::parsed(to_value(&request)?))?;

// Check status
let status = broker.read(&handle)?;

// Get response when ready
let response = broker.read(&handle.join(&path!("response")))?;
```

### HttpClientStore

HTTP client with a fixed base URL:

```rust
use structfs_http::HttpClientStore;

let mut client = HttpClientStore::new("https://api.example.com")?;

// GET /users/123
let record = client.read(&path!("users/123"))?;
```

## Types

### HttpRequest

```rust
pub struct HttpRequest {
    pub method: Method,           // GET, POST, PUT, DELETE, etc.
    pub path: String,             // URL or path
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Option<serde_json::Value>,
}

// Builder pattern
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

// Helper methods
response.is_success();      // 2xx
response.is_client_error(); // 4xx
response.is_server_error(); // 5xx
```

## Features

- `blocking` (default): Synchronous HTTP client using reqwest
