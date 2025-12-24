# structfs-http

HTTP stores for StructFS.

## Stores

### HttpBrokerStore

The broker store allows making HTTP requests to any URL using a deferred execution pattern:

1. **Write** an `HttpRequest` → returns a handle path
2. **Read** from the handle → executes the request and returns `HttpResponse`

```rust
use structfs_http::broker::HttpBrokerStore;
use structfs_http::{HttpRequest, HttpResponse};
use structfs_store::{Reader, Writer, path};

let mut broker = HttpBrokerStore::with_default_timeout()?;

// Queue a request
let request = HttpRequest::get("https://api.example.com/users/1")
    .with_header("Authorization", "Bearer token");
let handle = broker.write(&path!(""), &request)?;
// handle = "outstanding/0"

// Execute by reading
let response: HttpResponse = broker.read_owned(&handle)?.unwrap();
println!("Status: {}", response.status);
```

### HttpClientStore

HTTP client with a fixed base URL. Simpler but less flexible than the broker:

```rust
use structfs_http::blocking::HttpClientStore;

let mut client = HttpClientStore::new("https://api.example.com")?
    .with_default_header("Authorization", "Bearer token");

// GET /users/123
let user: User = client.read_owned(&path!("users/123"))?.unwrap();

// POST /users with body
client.write(&path!("users"), &new_user)?;
```

### RemoteStore

Connect to a remote StructFS server:

```rust
use structfs_http::RemoteStore;

let mut remote = RemoteStore::new("https://structfs.example.com")?;
let data: MyType = remote.read_owned(&path!("some/path"))?.unwrap();
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
response.json::<T>()?;      // Deserialize body
```

## Features

- `blocking` (default): Synchronous HTTP client using reqwest
- `async`: Async HTTP client (not yet implemented)
